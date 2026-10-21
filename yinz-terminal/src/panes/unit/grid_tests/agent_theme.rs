use super::*;
use crate::panes::terminal_character::{AnsiCode, NamedColor};
use yinz_types::agent::theme::*;
use yinz_types::colors::PaletteColor;

fn source(provider: &str, role: Role) -> AgentThemeSnapshot {
    AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: provider.into(),
        provider_version: "fixture".into(),
        theme: "fixture".into(),
        provenance: vec![],
        availability: Availability::Available,
        collisions: vec![],
        definitions: vec![Definition {
            token: "status".into(),
            channel: Channel::Foreground,
            color: SourceColor::Index(2),
            pair: None,
            target: Some(role.into()),
        }],
    }
}

fn style() -> Style {
    let mut style = Style::default();
    style.colors.exit_code_success.base = PaletteColor::Rgb((11, 22, 33));
    style.colors.exit_code_error.base = PaletteColor::Rgb((99, 88, 77));
    style
}

#[test]
fn semantic_mapping_precedes_selection_and_keeps_cells_and_capture_original() {
    for provider in ["claude", "codex"] {
        let mut grid = create_grid_with_content("");
        grid.update_theme(style().colors);
        event(&mut grid, "fixture", Some(source(provider, Role::Success)));
        feed_bytes(
            &mut grid,
            b"\x1b[1;3;4;7;32mX\x1b[0;38;5;2mY\x1b[38;2;1;2;3mZ",
        );
        let capture = grid.serialize(None).unwrap();
        let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
        let original = *chunks[0].terminal_characters[0].styles;
        assert_eq!(
            original.foreground,
            Some(AnsiCode::NamedColor(NamedColor::Green))
        );
        let mapped = chunks[0].application_styles(original);
        assert_eq!(mapped.foreground, Some(AnsiCode::RgbCode((11, 22, 33))));
        assert_eq!(
            (mapped.bold, mapped.italic, mapped.underline, mapped.reverse),
            (
                original.bold,
                original.italic,
                original.underline,
                original.reverse
            )
        );
        assert_eq!(
            chunks[0]
                .application_styles(*chunks[0].terminal_characters[1].styles)
                .foreground,
            mapped.foreground
        );
        assert_eq!(
            chunks[0]
                .application_styles(*chunks[0].terminal_characters[2].styles)
                .foreground,
            Some(AnsiCode::RgbCode((1, 2, 3)))
        );
        grid.start_selection(&Position::new(0, 0));
        grid.end_selection(&Position::new(0, 1));
        let selected = grid.render(0, 0, &style()).unwrap().unwrap().0;
        assert!(!selected[0].selection_and_colors().is_empty());
        assert_eq!(grid.serialize(None).unwrap(), capture);
        grid.set_agent_theme_mapping(AgentThemeMapping::Off);
        let original_chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
        assert_eq!(original_chunks[0].application_styles(original), original);
    }
}

#[test]
fn theme_preview_and_cancellation_repaint_cached_frames_without_recoloring_twice() {
    let mut grid = create_grid_with_content("");
    grid.update_theme(style().colors);
    event(&mut grid, "fixture", Some(source("codex", Role::Success)));
    feed_bytes(&mut grid, b"\x1b[32mresult");
    let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
    let original = *chunks[0].terminal_characters[0].styles;
    let mut preview = style();
    preview.colors.exit_code_success.base = PaletteColor::Rgb((66, 55, 44));
    grid.lock_renders();
    grid.update_theme(preview.colors);
    let changed = grid.render(0, 0, &preview).unwrap().unwrap().0;
    assert_eq!(
        changed[0].application_styles(original).foreground,
        Some(AnsiCode::RgbCode((66, 55, 44)))
    );
    grid.update_theme(style().colors);
    let cancelled = grid.render(0, 0, &style()).unwrap().unwrap().0;
    assert_eq!(
        cancelled[0].application_styles(original).foreground,
        Some(AnsiCode::RgbCode((11, 22, 33)))
    );
}

#[test]
fn preserved_scrollback_keeps_its_source_palette_across_restarts_and_reflow() {
    let mut grid = create_grid_with_content("");
    grid.update_theme(style().colors);
    event(&mut grid, "fixture", Some(source("claude", Role::Success)));
    feed_bytes(&mut grid, b"\x1b[32mold transcript");
    grid.reset_terminal_state_preserving_scrollback();
    grid.begin_agent_run();
    event(&mut grid, "fixture", Some(source("codex", Role::Error)));
    feed_bytes(&mut grid, b"\x1b[32mnew transcript");
    grid.force_change_size(20, 8);
    grid.move_viewport_up(grid.scrollback_buffer_lines);
    let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
    let old = chunks
        .iter()
        .find(|chunk| {
            chunk
                .terminal_characters
                .iter()
                .map(|c| c.character)
                .collect::<String>()
                .contains("old")
        })
        .unwrap();
    assert_eq!(
        old.application_styles(*old.terminal_characters[0].styles)
            .foreground,
        Some(AnsiCode::RgbCode((11, 22, 33)))
    );
    grid.set_agent_theme_mapping(AgentThemeMapping::Off);
    assert_eq!(
        old.application_styles(*old.terminal_characters[0].styles)
            .foreground,
        Some(AnsiCode::NamedColor(NamedColor::Green))
    );
}

#[test]
fn snapshot_osc_is_versioned_and_plain_panes_remain_unchanged() {
    let mut grid = create_grid_with_content("\x1b[32mplain");
    let before = grid.serialize(None).unwrap();
    let mut snapshot = source("claude", Role::Success);
    snapshot.version += 1;
    let encoded = BASE64_ENCODER.encode(
        serde_json::to_vec(&AgentThemeEvent {
            version: EVENT_VERSION,
            launch_id: "bad-version".into(),
            snapshot: Some(snapshot),
        })
        .unwrap(),
    );
    feed_bytes(
        &mut grid,
        format!("\x1b]777;yinz-agent-theme;{encoded}\x1b\\").as_bytes(),
    );
    let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
    assert!(chunks.iter().all(|chunk| chunk.agent_theme.is_none()));
    let original = *chunks[0].terminal_characters[0].styles;
    assert_eq!(chunks[0].application_styles(original), original);
    assert_eq!(grid.serialize(None).unwrap(), before);
}

#[test]
fn a_run_keeps_its_first_snapshot_until_the_next_managed_launch() {
    let mut grid = create_grid_with_content("");
    grid.update_theme(style().colors);
    event(&mut grid, "fixture", Some(source("claude", Role::Success)));
    event(&mut grid, "fixture", Some(source("claude", Role::Error)));
    feed_bytes(&mut grid, b"\x1b[32mfixed");
    let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
    assert_eq!(
        chunks[0]
            .application_styles(*chunks[0].terminal_characters[0].styles)
            .foreground,
        Some(AnsiCode::RgbCode((11, 22, 33)))
    );
    grid.reset_terminal_state();
    grid.begin_agent_run();
    feed_bytes(&mut grid, b"\x1b[32mshell");
    let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
    assert_eq!(
        chunks[0]
            .application_styles(*chunks[0].terminal_characters[0].styles)
            .foreground,
        Some(AnsiCode::NamedColor(NamedColor::Green))
    );
}

#[test]
fn diff_pairs_underline_and_blank_backgrounds_follow_dark_and_light_terminal_themes() {
    let mut grid = create_grid_with_content("");
    grid.update_theme(style().colors);
    let mut snapshot = source("codex", Role::Accent);
    let pair = Some((SourceColor::Index(2), SourceColor::Index(22)));
    snapshot.definitions.extend([
        Definition {
            token: "addition.foreground".into(),
            channel: Channel::Foreground,
            color: SourceColor::Index(2),
            pair,
            target: Some(Role::Success.into()),
        },
        Definition {
            token: "addition.background".into(),
            channel: Channel::Background,
            color: SourceColor::Index(22),
            pair,
            target: Some(Role::AdditionBackground.into()),
        },
        Definition {
            token: "underline".into(),
            channel: Channel::Underline,
            color: SourceColor::Index(2),
            pair: None,
            target: Some(Role::Error.into()),
        },
        Definition {
            token: "background".into(),
            channel: Channel::Background,
            color: SourceColor::Rgb(1, 2, 3),
            pair: None,
            target: Some(Role::Background.into()),
        },
    ]);
    event(&mut grid, "fixture", Some(snapshot));
    feed_bytes(
        &mut grid,
        b"\x1b[48;2;1;2;3m\x1b[2J\x1b[32;48;5;22;58;5;2m+\x1b[49mA",
    );
    let capture = grid.serialize(None).unwrap();
    for (background, diff_background) in [
        ((10, 20, 30), (10, 20, 31)),
        ((250, 240, 230), (207, 201, 195)),
    ] {
        grid.update_terminal_theme(Some(yinz_types::style::TerminalTheme {
            foreground: (200, 210, 220),
            background,
            ansi: [(66, 66, 66); 16],
        }));
        let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
        let diff = chunks
            .iter()
            .find(|c| c.terminal_characters.iter().any(|c| c.character == '+'))
            .unwrap();
        let cell = diff
            .terminal_characters
            .iter()
            .find(|c| c.character == '+')
            .unwrap();
        let mapped = diff.application_styles(*cell.styles);
        assert_eq!(mapped.foreground, Some(AnsiCode::RgbCode((11, 22, 33))));
        assert_eq!(mapped.background, Some(AnsiCode::RgbCode(diff_background)));
        assert_eq!(
            mapped.underline_color,
            Some(AnsiCode::RgbCode((99, 88, 77)))
        );
        let shared = diff
            .terminal_characters
            .iter()
            .find(|c| c.character == 'A')
            .unwrap();
        assert_eq!(
            diff.application_styles(*shared.styles).foreground,
            Some(AnsiCode::RgbCode((0, 255, 255)))
        );
        let blank = chunks.iter().find(|c| c.y == 1).unwrap();
        assert_eq!(
            blank
                .application_styles(*blank.terminal_characters[0].styles)
                .background,
            Some(AnsiCode::RgbCode(background))
        );
        assert_eq!(grid.serialize(None).unwrap(), capture);
    }
}

fn event(grid: &mut Grid, id: &str, snapshot: Option<AgentThemeSnapshot>) {
    let event = AgentThemeEvent {
        version: EVENT_VERSION,
        launch_id: id.into(),
        snapshot,
    };
    let encoded = BASE64_ENCODER.encode(serde_json::to_vec(&event).unwrap());
    feed_bytes(
        grid,
        format!("\x1b]777;yinz-agent-theme;{encoded}\x1b\\").as_bytes(),
    );
}

fn painted(grid: &mut Grid) -> Vec<(char, Option<AnsiCode>, Option<AnsiCode>)> {
    grid.output_buffer.update_all_lines();
    grid.render(0, 0, &style())
        .unwrap()
        .unwrap()
        .0
        .into_iter()
        .flat_map(|chunk| {
            chunk
                .terminal_characters
                .iter()
                .map(|cell| {
                    let styles = chunk.application_styles(*cell.styles);
                    (cell.character, styles.foreground, styles.background)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

const SUCCESS: Option<AnsiCode> = Some(AnsiCode::RgbCode((11, 22, 33)));
const ERROR: Option<AnsiCode> = Some(AnsiCode::RgbCode((99, 88, 77)));
const GREEN: Option<AnsiCode> = Some(AnsiCode::NamedColor(NamedColor::Green));

#[test]
fn successive_shell_launches_partial_redraws_and_stale_events() {
    let mut grid = create_grid_with_content("");
    grid.update_theme(style().colors);
    feed_bytes(&mut grid, b"\x1b[32mS");
    event(&mut grid, "one", Some(source("claude", Role::Success)));
    feed_bytes(&mut grid, b"AAA");
    event(&mut grid, "one", Some(source("claude", Role::Error))); // duplicate cannot change the palette
    feed_bytes(&mut grid, b"A");
    event(&mut grid, "one", None);
    feed_bytes(&mut grid, b"s");
    event(&mut grid, "two", Some(source("codex", Role::Error)));
    event(&mut grid, "one", None); // stale end cannot clear the new run
    feed_bytes(&mut grid, b"BBB");
    assert_eq!(
        painted(&mut grid)
            .iter()
            .take(9)
            .map(|c| c.1)
            .collect::<Vec<_>>(),
        vec![GREEN, SUCCESS, SUCCESS, SUCCESS, SUCCESS, GREEN, ERROR, ERROR, ERROR]
    );
    feed_bytes(&mut grid, b"\r\x1b[2CX"); // replace only one column of old output
    assert_eq!(painted(&mut grid)[2].1, ERROR);
    assert_eq!(painted(&mut grid)[3].1, SUCCESS);
    event(&mut grid, "two", None);
    feed_bytes(&mut grid, b"\r\x1b[32mshell-only");
    assert!(painted(&mut grid).iter().take(10).all(|c| c.1 == GREEN));
}

#[test]
fn insert_mode_assigns_ownership_beyond_the_materialized_row() {
    for prefix in ["A", "界"] {
        let mut grid = create_grid_with_content("");
        grid.update_theme(style().colors);
        feed_bytes(&mut grid, format!("\x1b[32m{prefix}").as_bytes());
        event(&mut grid, "one", Some(source("claude", Role::Success)));
        feed_bytes(&mut grid, b"\x1b[4h\x1b[4CX");
        let column = if prefix == "A" { 5 } else { 6 };
        assert!(grid.viewport[0].palettes.at(column).is_some());
        assert!(grid.viewport[0].palettes.at(column - 1).is_none());
        let cells = painted(&mut grid);
        assert!(cells.iter().any(|c| c.0 == 'X' && c.1 == SUCCESS));
        assert!(cells
            .iter()
            .any(|c| c.0 == prefix.chars().next().unwrap() && c.1 == GREEN));
    }
}

#[test]
fn handoff_padding_preserves_legacy_continuation_and_wrap() {
    for content in [
        "",
        "Hello",
        "0123456789",
        "0123456789H",
        "界Hi",
        "Hi\r\nthere",
    ] {
        let mut source_grid = create_grid_with_content("");
        source_grid.change_size(10, 10);
        event(
            &mut source_grid,
            "one",
            Some(source("claude", Role::Success)),
        );
        feed_bytes(&mut source_grid, b"\x1b[32m");
        feed_bytes(&mut source_grid, content.as_bytes());
        // Split the implicit padding between owners without materializing it.
        // Save/restore must span palette chunks as well as styles.
        event(&mut source_grid, "two", Some(source("codex", Role::Error)));
        let row = source_grid.viewport.back_mut().unwrap();
        row.palettes
            .set(row.width() + 1..usize::MAX, Some(&source_grid.agent_theme));
        let legacy = source_grid.serialize(Some(0)).unwrap();
        let (contents, metadata) = source_grid.serialize_handoff(Some(0), 100_000).unwrap();
        assert!(metadata.as_ref().unwrap().valid(&contents));
        let replay = crate::agent_theme::handoff_replay(contents, metadata);
        let mut expected = create_grid_with_content("");
        let mut adopted = create_grid_with_content("");
        for (grid, bytes) in [
            (&mut expected, legacy.as_bytes()),
            (&mut adopted, replay.as_slice()),
        ] {
            grid.change_size(10, 10);
            grid.set_agent_theme_mapping(AgentThemeMapping::Off);
            feed_bytes(grid, bytes);
            feed_bytes(grid, b"!");
        }
        assert_eq!(
            (adopted.cursor.x, adopted.cursor.y),
            (expected.cursor.x, expected.cursor.y),
            "{content:?}"
        );
        assert_eq!(painted(&mut adopted), painted(&mut expected), "{content:?}");
    }
}

#[test]
fn column_ownership_survives_wide_overwrite_insert_delete_and_erase() {
    let mut grid = create_grid_with_content("");
    grid.update_theme(style().colors);
    event(&mut grid, "one", Some(source("claude", Role::Success)));
    feed_bytes(&mut grid, "\x1b[32mA界BC".as_bytes());
    event(&mut grid, "two", Some(source("codex", Role::Error)));
    feed_bytes(&mut grid, b"\r\x1b[2CX"); // second half of wide character
    let cells = painted(&mut grid);
    assert_eq!(cells[0].1, SUCCESS);
    assert_eq!((cells[2].0, cells[2].1), ('X', ERROR));
    assert_eq!(cells[3].1, SUCCESS);
    feed_bytes(&mut grid, b"\r\x1b[1C\x1b[@"); // insert active blank before retained text
    let cells = painted(&mut grid);
    assert_eq!(cells[0].1, SUCCESS);
    assert!(grid.viewport[0].palettes.at(1).is_some());
    assert_eq!(cells[3].1, ERROR);
    assert_eq!(cells[4].1, SUCCESS);
    feed_bytes(&mut grid, b"\x1b[P");
    assert_eq!(painted(&mut grid)[3].1, SUCCESS);
    feed_bytes(&mut grid, b"\x1b[X");
    assert_eq!(painted(&mut grid)[1].1, ERROR);
    feed_bytes(&mut grid, b"\x1b[K");
    assert_eq!(painted(&mut grid)[0].1, SUCCESS);
    assert!(grid.viewport[0].palettes.at(40).is_some());
    event(&mut grid, "two", None);
    feed_bytes(&mut grid, b"\x1b[2K");
    assert!(grid.viewport[0].palettes.at(40).is_none());
}

#[test]
fn scrolling_reflow_and_alternate_screen_keep_content_owners() {
    let mut grid = create_grid_with_content("");
    grid.change_size(3, 8);
    grid.update_theme(style().colors);
    event(&mut grid, "one", Some(source("claude", Role::Success)));
    feed_bytes(&mut grid, b"\x1b[32mAAAA");
    event(&mut grid, "two", Some(source("codex", Role::Error)));
    feed_bytes(&mut grid, b"BBBB\r\nCCCC\r\nDDDD\r\nEEEE");
    grid.change_size(10, 4);
    grid.reset_viewport();
    let cells = painted(&mut grid);
    assert!(cells.iter().any(|c| c.0 == 'A' && c.1 == SUCCESS));
    assert!(cells
        .iter()
        .filter(|c| "BCDE".contains(c.0))
        .all(|c| c.1 == ERROR));
    feed_bytes(&mut grid, b"\x1b[?1049h\x1b[32mALT");
    event(&mut grid, "two", None);
    feed_bytes(&mut grid, b"\x1b[?1049l");
    let cells = painted(&mut grid);
    assert!(cells.iter().any(|c| c.0 == 'A' && c.1 == SUCCESS));
    feed_bytes(&mut grid, b"\x1b[32mS");
    assert!(painted(&mut grid)
        .iter()
        .any(|c| c.0 == 'S' && c.1 == GREEN));
}

#[test]
fn mixed_handoff_restores_history_before_active_launch_and_recompiles_all_palettes() {
    let mut source_grid = create_grid_with_content("");
    source_grid.change_size(10, 12);
    source_grid.update_theme(style().colors);
    feed_bytes(&mut source_grid, b"\x1b[32mS");
    event(
        &mut source_grid,
        "one",
        Some(source("claude", Role::Success)),
    );
    feed_bytes(&mut source_grid, b"AAA");
    event(&mut source_grid, "one", None);
    feed_bytes(&mut source_grid, b"s");
    event(&mut source_grid, "two", Some(source("codex", Role::Error)));
    feed_bytes(&mut source_grid, b"BBB");
    let (contents, metadata) = source_grid.serialize_handoff(Some(0), 100_000).unwrap();
    assert!(!contents.contains("yinz-agent"));
    assert_eq!(metadata.as_ref().unwrap().snapshots.len(), 2);
    assert!(metadata.as_ref().unwrap().valid(&contents));
    let replay = crate::agent_theme::handoff_replay(contents, metadata);
    let mut adopted = create_grid_with_content("");
    adopted.change_size(10, 12);
    adopted.update_theme(style().colors);
    feed_bytes(&mut adopted, &replay);
    feed_bytes(&mut adopted, b"\r\n\x1b[32mN");
    for (mapping, success, error) in [
        (AgentThemeMapping::Semantic, SUCCESS, ERROR),
        (AgentThemeMapping::Off, GREEN, GREEN),
    ] {
        adopted.set_agent_theme_mapping(mapping);
        let cells = painted(&mut adopted);
        for (character, expected) in [
            ('S', GREEN),
            ('A', success),
            ('s', GREEN),
            ('B', error),
            ('N', error),
        ] {
            assert!(
                cells.iter().any(|c| c.0 == character && c.1 == expected),
                "{character}: {cells:?}"
            );
        }
    }
    adopted.set_agent_theme_mapping(AgentThemeMapping::Semantic);
    let mut changed = style().colors;
    changed.exit_code_success.base = PaletteColor::Rgb((55, 66, 77));
    adopted.update_theme(changed);
    assert!(painted(&mut adopted)
        .iter()
        .any(|c| c.0 == 'A' && c.1 == Some(AnsiCode::RgbCode((55, 66, 77)))));
    event(&mut adopted, "one", None);
    assert_eq!(adopted.agent_launch.as_ref().unwrap().launch_id, "two");
    event(&mut adopted, "two", None);
    feed_bytes(&mut adopted, b"\x1b[32mZ");
    assert!(painted(&mut adopted)
        .iter()
        .any(|c| c.0 == 'Z' && c.1 == GREEN));
}

#[test]
fn handoff_budget_trims_content_and_metadata_together_and_bad_metadata_is_ignored() {
    let mut grid = create_grid_with_content("");
    grid.change_size(4, 10);
    for n in 0..20 {
        event(
            &mut grid,
            &n.to_string(),
            Some(source(
                "claude",
                if n % 2 == 0 {
                    Role::Success
                } else {
                    Role::Error
                },
            )),
        );
        feed_bytes(&mut grid, format!("\x1b[32mrow{n:02}\r\n").as_bytes());
    }
    let (full, _) = grid.serialize_handoff(Some(0), 100_000).unwrap();
    let (contents, metadata) = grid.serialize_handoff(Some(0), 2000).unwrap();
    assert!(contents.len() < full.len());
    let metadata = metadata.unwrap();
    assert!(contents.len() + serde_json::to_vec(&metadata).unwrap().len() <= 2000);
    assert!(metadata.valid(&contents));
    assert!(contents.contains("row19"));
    let mut adopted = create_grid_with_content("");
    adopted.update_theme(style().colors);
    feed_bytes(
        &mut adopted,
        &crate::agent_theme::handoff_replay(contents.clone(), Some(metadata.clone())),
    );
    assert!(painted(&mut adopted)
        .iter()
        .any(|c| c.0 == '9' && c.1 == ERROR));
    let mut malformed = metadata;
    malformed.associations[0].offset = usize::MAX;
    assert_eq!(
        crate::agent_theme::handoff_replay(contents.clone(), Some(malformed)),
        contents.as_bytes()
    );
    assert_eq!(
        crate::agent_theme::handoff_replay(contents.clone(), None),
        contents.as_bytes()
    );
}

#[test]
fn blank_background_spans_and_locked_frames_recompile_without_reassigning_ownership() {
    let mut grid = create_grid_with_content("");
    grid.change_size(3, 8);
    grid.update_theme(style().colors);
    let palette = |role| {
        let mut palette = source("claude", role);
        palette.definitions.push(Definition {
            token: "surface".into(),
            channel: Channel::Background,
            color: SourceColor::Index(2),
            pair: None,
            target: Some(role.into()),
        });
        palette
    };
    event(&mut grid, "one", Some(palette(Role::Success)));
    feed_bytes(&mut grid, b"\x1b[32;42m\x1b[2JAAAA");
    event(&mut grid, "two", Some(palette(Role::Error)));
    feed_bytes(&mut grid, b"\x1b[K");
    let cells = painted(&mut grid);
    assert_eq!(cells[0].2, SUCCESS);
    assert_eq!(cells[6].2, ERROR);
    assert_eq!(cells[8].2, SUCCESS);
    grid.lock_renders();
    feed_bytes(&mut grid, b"\rBBBB");
    let mut changed = style().colors;
    changed.exit_code_success.base = PaletteColor::Rgb((1, 2, 3));
    grid.update_theme(changed);
    let chunks = grid.render(0, 0, &style()).unwrap().unwrap().0;
    assert_eq!(chunks[0].terminal_characters[0].character, 'A');
    assert_eq!(
        chunks[0]
            .application_styles(*chunks[0].terminal_characters[0].styles)
            .foreground,
        Some(AnsiCode::RgbCode((1, 2, 3)))
    );
    let suffix = chunks
        .iter()
        .find(|chunk| chunk.y == 0 && chunk.x == 4)
        .unwrap();
    assert_eq!(
        suffix
            .application_styles(*suffix.terminal_characters[0].styles)
            .background,
        ERROR
    );
}

fn codex_palette() -> AgentThemeSnapshot {
    let mut snapshot = source("codex", Role::Success);
    snapshot.provider_version = "0.162.1".into();
    snapshot
}

fn terminal_style(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> Style {
    let mut style = style();
    style.colors.text_unselected.base = PaletteColor::Rgb(fg);
    style.colors.text_unselected.background = PaletteColor::Rgb(bg);
    style
}

#[test]
fn codex_local_queries_enrich_existing_cells_frames_and_handoff_without_output() {
    let mut grid = create_grid_with_content("");
    grid.change_size(3, 12);
    let dark = terminal_style((220, 220, 220), (18, 20, 30));
    grid.update_theme(dark.colors);
    event(&mut grid, "codex-run", Some(codex_palette()));
    // Render before observation to prove enrichment updates retained palette references.
    feed_bytes(&mut grid, b"\x1b[48;2;46;48;57m\x1b[2JCOMPOSER");
    let retained = grid.render(0, 0, &dark).unwrap().unwrap().0;
    let raw = *retained[0].terminal_characters[0].styles;
    assert_eq!(
        retained[0].application_styles(raw).background,
        raw.background
    );
    feed_bytes(
        &mut grid,
        b"\x1b]10;rgb:dcdc/dcdc/dcdc\x07\x1b]11;rgb:1212/1414/1e1e\x07\x1b]10;?;?\x07",
    );
    assert_eq!(grid.pending_messages_to_pty.len(), 2);
    assert!(grid.pending_forwarded_queries.is_empty());
    assert_eq!(
        retained[0].application_styles(raw).background,
        Some(AnsiCode::RgbCode((34, 36, 45)))
    );
    let capture = grid.serialize(None).unwrap();
    let learned = grid.agent_launch.as_ref().unwrap().snapshot.clone();
    let updates = std::mem::take(&mut grid.pending_agent_theme_updates);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].launch_id, "codex-run");
    assert!(!updates[0]
        .snapshot
        .as_ref()
        .unwrap()
        .definitions
        .iter()
        .any(|d| d.token == "codex.terminal.composer-plan"));
    assert_eq!(updates[1].snapshot.as_ref(), Some(&learned));
    feed_bytes(&mut grid, b"\x1b]10;?;?\x07");
    assert!(grid.pending_agent_theme_updates.is_empty());
    grid.lock_renders();
    for (theme, expected) in [
        (
            terminal_style((20, 20, 20), (255, 255, 255)),
            (236, 236, 236),
        ),
        (dark, (34, 36, 45)), // preview cancellation
    ] {
        grid.update_theme(theme.colors);
        let chunks = grid.render(0, 0, &theme).unwrap().unwrap().0;
        for chunk in &chunks {
            for cell in &chunk.terminal_characters {
                assert_eq!(
                    chunk.application_styles(*cell.styles).background,
                    Some(AnsiCode::RgbCode(expected))
                );
            }
        }
        assert_eq!(
            retained[0].application_styles(raw).background,
            Some(AnsiCode::RgbCode(expected))
        );
        assert_eq!(grid.agent_launch.as_ref().unwrap().snapshot, learned);
        assert_eq!(grid.serialize(None).unwrap(), capture);
    }
    grid.unlock_renders();
    feed_bytes(
        &mut grid,
        b"\r\n\x1b[48;2;55;57;66mSUBMITTED\r\nPLAN\r\nSCROLL",
    );
    let (contents, metadata) = grid.serialize_handoff(Some(0), 100_000).unwrap();
    let metadata = metadata.unwrap();
    assert_eq!(metadata.snapshots, vec![learned]);
    let mut adopted = create_grid_with_content("");
    adopted.change_size(10, 12);
    adopted.update_theme(terminal_style((20, 20, 20), (255, 255, 255)).colors);
    feed_bytes(
        &mut adopted,
        &crate::agent_theme::handoff_replay(contents, Some(metadata)),
    );
    let cells = painted(&mut adopted);
    for label in ['C', 'S', 'P'] {
        assert!(cells
            .iter()
            .any(|c| c.0 == label && c.2 == Some(AnsiCode::RgbCode((236, 236, 236)))));
    }
    adopted.set_agent_theme_mapping(AgentThemeMapping::Off);
    let cells = painted(&mut adopted);
    assert!(cells
        .iter()
        .any(|c| c.0 == 'C' && c.2 == Some(AnsiCode::RgbCode((46, 48, 57)))));
    assert!(cells
        .iter()
        .any(|c| c.0 == 'S' && c.2 == Some(AnsiCode::RgbCode((55, 57, 66)))));
}

#[test]
fn codex_forwarded_observations_ignore_invalid_missing_and_previous_launch_replies() {
    let mut grid = create_grid_with_content("");
    event(&mut grid, "old", Some(codex_palette()));
    feed_bytes(&mut grid, b"\x1b]10;?\x07\x1b]11;?\x07");
    let old = std::mem::take(&mut grid.pending_forwarded_queries);
    event(&mut grid, "new", Some(codex_palette()));
    feed_bytes(&mut grid, b"\x1b]10;?\x07\x1b]11;?\x07");
    let current = std::mem::take(&mut grid.pending_forwarded_queries);
    for (query, reply) in old.iter().zip([
        b"\x1b]10;rgb:ffff/ffff/ffff\x07".as_slice(),
        b"\x1b]11;rgb:0000/0000/0000\x07",
    ]) {
        grid.record_terminal_color_reply(query, reply);
    }
    assert_eq!(
        grid.agent_launch.as_ref().unwrap().snapshot,
        codex_palette()
    );
    grid.record_terminal_color_reply(&current[0], b"\x1b]11;rgb:ffff/ffff/ffff\x07");
    grid.record_terminal_color_reply(&current[1], b"");
    assert!(grid.pending_agent_theme_updates.is_empty());
    assert_eq!(
        grid.agent_launch.as_ref().unwrap().snapshot,
        codex_palette()
    );
    feed_bytes(&mut grid, b"\x1b]10;?\x07\x1b]11;?\x07");
    let queries = std::mem::take(&mut grid.pending_forwarded_queries);
    grid.record_terminal_color_reply(&queries[0], b"\x1b]10;rgb:dcdc/dcdc/dcdc\x07");
    assert!(!grid
        .agent_launch
        .as_ref()
        .unwrap()
        .snapshot
        .definitions
        .iter()
        .any(|d| d.target == Some(Role::Surface.into())));
    grid.update_theme(terminal_style((0, 0, 0), (255, 255, 255)).colors);
    grid.record_terminal_color_reply(&queries[1], b"\x1b]11;rgb:1212/1414/1e1e\x07");
    feed_bytes(&mut grid, b"\x1b[48;2;46;48;57mX");
    assert!(painted(&mut grid)
        .iter()
        .any(|c| c.0 == 'X' && c.2 == Some(AnsiCode::RgbCode((235, 235, 235)))));
}

#[test]
fn claude_auto_inline_code_recolors_without_output_and_preserves_bold_and_capture() {
    let mut palette = source("claude", Role::Accent);
    palette.theme = "auto".into();
    palette.definitions = [(87, 105, 247), (177, 185, 249)]
        .into_iter()
        .map(|(r, g, b)| Definition {
            token: "permission".into(),
            channel: Channel::Foreground,
            color: SourceColor::Rgb(r, g, b),
            pair: None,
            target: Some(Role::Accent.into()),
        })
        .collect();
    let mut grid = create_grid_with_content("");
    event(&mut grid, "claude-auto", Some(palette));
    feed_bytes(
        &mut grid,
        b"\x1b[38;2;87;105;247mL\x1b[1mB\x1b[0;38;2;177;185;249mD\x1b[1mB\x1b[0m",
    );
    let capture = grid.serialize(None).unwrap();
    assert!(capture.contains("38;2;87;105;247"));
    assert!(capture.contains("38;2;177;185;249"));
    for (fg, bg, accent, mapping) in [
        (200, 0, (11, 22, 33), AgentThemeMapping::Semantic),
        (0, 250, (44, 55, 66), AgentThemeMapping::Semantic),
        (0, 250, (44, 55, 66), AgentThemeMapping::Off),
    ] {
        let mut theme = terminal_style((fg, fg, fg), (bg, bg, bg));
        theme.colors.text_unselected.emphasis_1 = PaletteColor::Rgb(accent);
        grid.update_theme(theme.colors);
        grid.set_agent_theme_mapping(mapping);
        let chunks = grid.render(0, 0, &theme).unwrap().unwrap().0;
        let mut checked = 0;
        for chunk in &chunks {
            for cell in &chunk.terminal_characters {
                if !matches!(cell.character, 'L' | 'D' | 'B') {
                    continue;
                }
                let original = *cell.styles;
                let mapped = chunk.application_styles(original);
                assert_eq!(
                    mapped.foreground,
                    if mapping == AgentThemeMapping::Off {
                        original.foreground
                    } else {
                        Some(AnsiCode::RgbCode(accent))
                    }
                );
                assert_eq!(mapped.bold, original.bold);
                assert_eq!(mapped.bold == Some(AnsiCode::On), cell.character == 'B');
                checked += 1;
            }
        }
        assert_eq!(checked, 4);
        assert_eq!(grid.serialize(None).unwrap(), capture);
    }
}

#[test]
fn claude_ui_and_diffs_recolor_existing_content_and_preserve_capture() {
    let mut palette = source("claude", Role::Accent);
    palette.definitions.clear();
    let cases = [
        (
            "briefLabelYou",
            Channel::Foreground,
            Role::Accent,
            (37, 99, 235),
        ),
        (
            "clawd_body",
            Channel::Foreground,
            Role::Accent,
            (215, 119, 87),
        ),
        (
            "rate_limit_empty",
            Channel::Background,
            Role::Subtle,
            (39, 47, 111),
        ),
        (
            "diffAddedWord",
            Channel::Foreground,
            Role::Success,
            (47, 157, 68),
        ),
        (
            "diffRemovedWord",
            Channel::Foreground,
            Role::Error,
            (209, 69, 75),
        ),
        (
            "diffAdded",
            Channel::Background,
            Role::AdditionBackground,
            (105, 219, 124),
        ),
        (
            "diffRemoved",
            Channel::Background,
            Role::RemovalBackground,
            (255, 168, 180),
        ),
        (
            "diffAddedDimmed",
            Channel::Background,
            Role::AdditionDimmed,
            (199, 225, 203),
        ),
        (
            "diffRemovedDimmed",
            Channel::Background,
            Role::RemovalDimmed,
            (253, 210, 216),
        ),
        (
            "diffAddedWord",
            Channel::Background,
            Role::AdditionWord,
            (47, 157, 68),
        ),
        (
            "diffRemovedWord",
            Channel::Background,
            Role::RemovalWord,
            (209, 69, 75),
        ),
        (
            "rate_limit_fill",
            Channel::Foreground,
            Role::Accent,
            (87, 105, 247),
        ),
    ];
    for (token, channel, role, (r, g, b)) in cases {
        palette.definitions.push(Definition {
            token: token.into(),
            channel,
            color: SourceColor::Rgb(r, g, b),
            pair: None,
            target: Some(role.into()),
        });
    }
    // The mascot shares Claude's accent; identities with that color follow it too.
    palette.definitions.push(Definition {
        token: "identity".into(),
        channel: Channel::Foreground,
        color: SourceColor::Rgb(215, 119, 87),
        pair: None,
        target: None,
    });
    let mut grid = create_grid_with_content("");
    event(&mut grid, "claude-ui", Some(palette));
    for (i, (_, channel, _, (r, g, b))) in cases.iter().enumerate() {
        let channel = if *channel == Channel::Foreground {
            38
        } else {
            48
        };
        feed_bytes(
            &mut grid,
            format!(
                "\x1b[0;{channel};2;{r};{g};{b}m{}",
                (b'A' + i as u8) as char
            )
            .as_bytes(),
        );
    }
    feed_bytes(
        &mut grid,
        "\x1b[0;38;2;87;105;247;48;2;39;47;111m▌".as_bytes(),
    );
    let capture = grid.serialize(None).unwrap();
    assert!(capture.contains("48;2;39;47;111"));
    assert!(capture.contains("38;2;87;105;247"));
    assert!(capture.contains("48;2;47;157;68"));
    assert!(capture.contains("38;2;47;157;68"));
    for (fg, bg, expected) in [
        (200, 0, [50, 50, 80, 100, 200, 18, 36, 10, 20, 30, 60, 50]),
        (
            0,
            250,
            [150, 150, 150, 100, 200, 223, 241, 235, 245, 205, 235, 150],
        ),
    ] {
        let mut theme = terminal_style((fg, fg, fg), (bg, bg, bg));
        theme.colors.text_unselected.emphasis_1 =
            PaletteColor::Rgb((expected[0], expected[0], expected[0]));
        theme.colors.exit_code_success.base = PaletteColor::Rgb((100, 100, 100));
        theme.colors.exit_code_error.base = PaletteColor::Rgb((200, 200, 200));
        grid.update_theme(theme.colors);
        let cells = painted(&mut grid);
        for (i, (_, channel, _, _)) in cases.iter().enumerate() {
            let actual = if *channel == Channel::Foreground {
                cells[i].1
            } else {
                cells[i].2
            };
            let value = expected[i];
            assert_eq!(
                actual,
                Some(AnsiCode::RgbCode((value, value, value))),
                "{}",
                cases[i].0
            );
        }
        let accent = expected[0];
        let subtle = expected[2];
        assert_eq!(
            cells[cases.len()],
            (
                '▌',
                Some(AnsiCode::RgbCode((accent, accent, accent))),
                Some(AnsiCode::RgbCode((subtle, subtle, subtle))),
            )
        );
        assert_eq!(grid.serialize(None).unwrap(), capture);
    }
    grid.set_agent_theme_mapping(AgentThemeMapping::Off);
    let cells = painted(&mut grid);
    for (i, (_, channel, _, rgb)) in cases.iter().enumerate() {
        let actual = if *channel == Channel::Foreground {
            cells[i].1
        } else {
            cells[i].2
        };
        assert_eq!(actual, Some(AnsiCode::RgbCode(*rgb)));
    }
    assert_eq!(
        cells[cases.len()],
        (
            '▌',
            Some(AnsiCode::RgbCode((87, 105, 247))),
            Some(AnsiCode::RgbCode((39, 47, 111))),
        )
    );
    assert_eq!(grid.serialize(None).unwrap(), capture);
}

#[test]
fn claude_code_renderer_recolors_retained_file_diff_and_syntax_output() {
    let palette = serde_json::from_str(include_str!(
        "../../../../../yinz-process/src/agent_theme/unit/fixtures/claude-light-code.json"
    ))
    .unwrap();
    let mut grid = create_grid_with_content("");
    event(&mut grid, "claude-code", Some(palette));
    feed_bytes(
        &mut grid,
        include_bytes!(
            "../../../../../yinz-process/src/agent_theme/unit/fixtures/claude-light-code.ansi"
        ),
    );
    let capture = grid.serialize(None).unwrap();
    assert!(capture.contains("38;2;51;51;51"));
    assert!(capture.contains("48;2;220;255;220"));
    assert!(capture.contains("38;5;246"));

    // Repaint both ways, then opt out, without feeding another PTY byte.
    for (fg, bg, accent, muted, add, remove, add_word, remove_word, mapping) in [
        (200, 0, 50, 130, 18, 36, 30, 60, AgentThemeMapping::Semantic),
        (
            0,
            250,
            150,
            88,
            223,
            241,
            205,
            235,
            AgentThemeMapping::Semantic,
        ),
        (200, 0, 50, 130, 18, 36, 30, 60, AgentThemeMapping::Semantic),
        (200, 0, 50, 130, 18, 36, 30, 60, AgentThemeMapping::Off),
    ] {
        let mut theme = terminal_style((fg, fg, fg), (bg, bg, bg));
        theme.colors.text_unselected.emphasis_1 = PaletteColor::Rgb((accent, accent, accent));
        theme.colors.exit_code_success.base = PaletteColor::Rgb((100, 100, 100));
        theme.colors.exit_code_error.base = PaletteColor::Rgb((200, 200, 200));
        grid.update_theme(theme.colors);
        grid.set_agent_theme_mapping(mapping);
        let chunks = grid.render(0, 0, &theme).unwrap().unwrap().0;
        let cells = chunks
            .iter()
            .flat_map(|chunk| {
                chunk.terminal_characters.iter().map(|cell| {
                    let original = *cell.styles;
                    let mapped = chunk.application_styles(original);
                    let mut attributes = mapped;
                    attributes.foreground = original.foreground;
                    attributes.background = original.background;
                    assert_eq!(attributes, original);
                    if mapping == AgentThemeMapping::Off {
                        assert_eq!(mapped, original);
                    }
                    (cell.character, mapped)
                })
            })
            .collect::<Vec<_>>();
        let text = cells.iter().map(|(c, _)| c).collect::<String>();
        for (label, foreground, background) in [
            ("plain alpha", fg, bg),
            ("unchanged", fg, bg),
            ("-", 200, remove),
            ("removed", fg, remove),
            ("old", fg, remove_word),
            ("+", 100, add),
            ("added", fg, add),
            ("new", fg, add_word),
            ("const", accent, bg),
            ("identifier", accent, bg),
            ("=", accent, bg),
            ("\"string\"", 100, bg),
            ("42", accent, bg),
            (";", fg, bg),
            ("// comment", muted, bg),
            ("indexed-string", 100, bg),
            ("indexed-comment", muted, bg),
        ] {
            let offset = text.find(label).unwrap();
            let styles = cells[text[..offset].chars().count()].1;
            if mapping == AgentThemeMapping::Semantic {
                assert_eq!(
                    styles.foreground,
                    Some(AnsiCode::RgbCode((foreground, foreground, foreground))),
                    "{label}"
                );
                assert_eq!(
                    styles.background,
                    Some(AnsiCode::RgbCode((background, background, background))),
                    "{label}"
                );
            }
            if label == "const" {
                assert_eq!(styles.bold, Some(AnsiCode::On));
            }
            if label == "\"string\"" {
                assert_eq!(styles.italic, Some(AnsiCode::On));
            }
            if label == "// comment" {
                assert_eq!(styles.dim, Some(AnsiCode::On));
            }
        }
        assert_eq!(grid.serialize(None).unwrap(), capture);
    }
}

#[test]
fn wide_character_erasure_updates_only_replaced_palette_columns() {
    let old = Rc::new(crate::agent_theme::SourcePalette::new(
        Some(source("old", Role::Success)),
        Style::default(),
    ));
    let new = Rc::new(crate::agent_theme::SourcePalette::new(
        Some(source("new", Role::Error)),
        Style::default(),
    ));
    for erase_beginning in [false, true] {
        for cursor in [2, 3] {
            let mut row = Row::from_columns("ab你好".chars().map(TerminalCharacter::new).collect());
            row.palettes.set(0..6, Some(&old));
            row.write_palette = Some(new.clone());
            if erase_beginning {
                row.replace_and_pad_beginning(
                    cursor,
                    crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER,
                );
            } else {
                row.replace_and_pad_end(
                    cursor,
                    6,
                    crate::panes::terminal_character::EMPTY_TERMINAL_CHARACTER,
                );
            }
            for column in 0..6 {
                let replaced = if erase_beginning {
                    column < 4
                } else {
                    column >= 2
                };
                assert!(Rc::ptr_eq(
                    row.palettes.at(column).unwrap(),
                    if replaced { &new } else { &old }
                ));
            }
        }
    }
}
