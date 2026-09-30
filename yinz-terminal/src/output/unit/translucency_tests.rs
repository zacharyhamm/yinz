use super::*;
use yinz_types::{colors::PaletteColor, pane_size::Dimension};

const FG: PaletteColor = PaletteColor::Rgb((200, 160, 120));
const BG: PaletteColor = PaletteColor::Rgb((20, 40, 60));
const TINT: PaletteColor = PaletteColor::Rgb((100, 100, 100));

fn rect(x: usize, cols: usize) -> CellRect {
    CellRect {
        x,
        y: 0,
        cols,
        rows: 1,
    }
}
fn chunk(text: &str, x: usize) -> CharacterChunk {
    CharacterChunk::new(text.chars().map(TerminalCharacter::new).collect(), x, 0)
}
fn text(chunks: &[CharacterChunk]) -> String {
    chunks
        .iter()
        .flat_map(|c| c.terminal_characters.iter().map(|c| c.character))
        .collect()
}
fn compose(opacity: u8, under: CharacterChunk, over: CharacterChunk) -> Vec<CharacterChunk> {
    let mut composition =
        translucency::Composition::new(vec![(1, rect(0, 5), opacity)], FG, BG, TINT, [None; 256]);
    composition.add(&[under], 0);
    composition.add(&[over], 1);
    composition.finish()
}

#[test]
fn translucent_blank_cells_blend_both_colors_and_leave_panel_lettering_solid() {
    for (opacity, fg, bg) in [
        (0, FG, BG),
        (
            50,
            PaletteColor::Rgb((150, 130, 110)),
            PaletteColor::Rgb((60, 70, 80)),
        ),
    ] {
        let result = compose(opacity, chunk("abcde", 0), chunk("  X  ", 0));
        assert_eq!(text(&result), "abXde");
        let cells = &result[0].terminal_characters;
        assert_eq!(cells[0].styles.foreground, Some(fg.into()));
        assert_eq!(cells[0].styles.background, Some(bg.into()));
        assert_eq!(cells[2].styles.foreground, Some(FG.into()));
        assert_eq!(cells[2].styles.background, Some(TINT.into()));
    }
}

#[test]
fn explicit_backgrounds_reverse_video_and_highlights_protect_blank_controls() {
    let mut over = chunk("     ", 0);
    over.terminal_characters[0].styles = DEFAULT_STYLES
        .background(Some(AnsiCode::ColorIndex(4)))
        .into();
    over.terminal_characters[1].styles = DEFAULT_STYLES.reverse(Some(AnsiCode::On)).into();
    let mut selection = Selection::default();
    selection.set_start_and_end_positions(
        yinz_types::position::Position::new(0, 2),
        yinz_types::position::Position::new(0, 3),
    );
    over.add_selection_and_colors(
        HighlightSelection {
            selection,
            bg: Some(AnsiCode::RgbCode((1, 2, 3))),
            fg: None,
            bold: false,
            italic: false,
            underline: false,
            layer: HighlightLayer::Hint,
        },
        0,
        0,
    );
    let result = compose(0, chunk("abcde", 0), over);
    assert_eq!(text(&result), "   de");
    assert_eq!(
        result[0].terminal_characters[2].styles.background,
        Some(AnsiCode::RgbCode((1, 2, 3)))
    );
}

#[test]
fn colors_are_resolved_before_blending_including_palette_defaults_and_reverse() {
    use crate::panes::terminal_character::NamedColor;
    let mut under = chunk("abcde", 0);
    under.pane_default_fg = Some(AnsiCode::RgbCode((40, 60, 80)));
    under.pane_default_bg = Some(AnsiCode::RgbCode((80, 60, 40)));
    let mut changed = [None; 256];
    changed[1] = Some(AnsiCode::RgbCode((0, 20, 40)));
    under.changed_colors = Some(changed);
    under.terminal_characters[0].styles = DEFAULT_STYLES
        .foreground(Some(AnsiCode::NamedColor(NamedColor::Red)))
        .reverse(Some(AnsiCode::On))
        .into();
    under.terminal_characters[1].styles = DEFAULT_STYLES
        .foreground(Some(AnsiCode::ColorIndex(1)))
        .into();
    let result = compose(50, under, chunk("     ", 0));
    let cells = &result[0].terminal_characters;
    assert_eq!(
        cells[0].styles.foreground,
        Some(AnsiCode::RgbCode((90, 80, 70)))
    );
    assert_eq!(
        cells[0].styles.background,
        Some(AnsiCode::RgbCode((50, 60, 70)))
    );
    assert_eq!(cells[0].styles.reverse, Some(AnsiCode::Reset));
    assert_eq!(
        cells[1].styles.foreground,
        Some(AnsiCode::RgbCode((50, 60, 70)))
    );
    assert_eq!(
        cells[2].styles.foreground,
        Some(AnsiCode::RgbCode((70, 80, 90)))
    );
}

#[test]
fn overlapping_layers_compose_in_order_even_when_chunks_arrive_out_of_order() {
    let mut composition = translucency::Composition::new(
        vec![(1, rect(0, 5), 50), (2, rect(0, 3), 50)],
        FG,
        BG,
        TINT,
        [None; 256],
    );
    composition.add(&[chunk(" X ", 0)], 2);
    composition.add(&[chunk("abcde", 0)], 0);
    composition.add(&[chunk("     ", 0)], 1);
    composition.add(&[chunk("Z", 4)], 3);
    let result = composition.finish();
    assert_eq!(text(&result), "aXcd");
    assert_eq!(
        result[0].terminal_characters[0].styles.foreground,
        Some(AnsiCode::RgbCode((125, 115, 105)))
    );
}

#[test]
fn wide_and_combining_glyphs_need_their_complete_visible_span() {
    assert_eq!(
        text(&compose(50, chunk("界e\u{301}xy", 0), chunk("     ", 0))),
        "界e\u{301}xy"
    );
    assert_eq!(
        text(&compose(50, chunk("界e\u{301}xy", 0), chunk(" X   ", 0))),
        " Xe\u{301}xy"
    );
    assert_eq!(
        text(&compose(50, chunk("界e\u{301}xy", 0), chunk("X    ", 0))),
        "X e\u{301}xy"
    );
    assert_eq!(
        text(&compose(50, chunk("界e\u{301}xy", 0), chunk("  Z  ", 0))),
        "界Zxy"
    );
    let mut composition =
        translucency::Composition::new(vec![(1, rect(1, 2), 0)], FG, BG, TINT, [None; 256]);
    composition.add(&[chunk("界界", 0)], 0);
    composition.add(&[chunk("  ", 1)], 1);
    assert_eq!(text(&composition.finish()), "  ");
}

#[test]
fn compositions_are_per_client_and_finish_below_screen_notifications() {
    let clients = HashSet::from([ClientId::new(1), ClientId::new(2)]);
    let mut output = Output::default();
    output.add_clients(
        &clients,
        Rc::new(RefCell::new(LinkHandler::new())),
        Some(FloatingPanesStack {
            layers: vec![PaneGeom {
                x: 0,
                y: 0,
                cols: Dimension::fixed(5),
                rows: Dimension::fixed(1),
                ..Default::default()
            }],
        }),
    );
    output.begin_translucent_composition(
        &clients,
        vec![(1, rect(0, 5), 50)],
        FG,
        BG,
        TINT,
        &HashMap::new(),
    );
    for client in &clients {
        output
            .add_character_chunks_to_client(
                *client,
                vec![chunk(
                    if *client == ClientId::new(1) {
                        "first"
                    } else {
                        "other"
                    },
                    0,
                )],
                None,
            )
            .unwrap();
        output
            .add_character_chunks_to_client(*client, vec![chunk("     ", 0)], Some(1))
            .unwrap();
    }
    output.finish_translucent_composition();
    assert_eq!(
        text(&output.client_character_chunks[&ClientId::new(1)]),
        "first"
    );
    assert_eq!(
        text(&output.client_character_chunks[&ClientId::new(2)]),
        "other"
    );
    output.set_floating_panes_stack(None);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk("toast", 0)], None)
        .unwrap();
    let bytes = output.serialize().unwrap();
    assert!(
        bytes[&ClientId::new(1)].find("first").unwrap()
            < bytes[&ClientId::new(1)].find("toast").unwrap()
    );
}

#[test]
fn opaque_output_keeps_the_original_clipping_path_and_graphics_stay_clipped() {
    let client = ClientId::new(1);
    let mut output = Output::default();
    output.add_clients(
        &HashSet::from([client]),
        Rc::new(RefCell::new(LinkHandler::new())),
        Some(FloatingPanesStack {
            layers: vec![PaneGeom {
                cols: Dimension::fixed(5),
                rows: Dimension::fixed(1),
                ..Default::default()
            }],
        }),
    );
    output
        .add_character_chunks_to_client(client, vec![chunk("under", 0)], None)
        .unwrap();
    output
        .add_character_chunks_to_client(client, vec![chunk("     ", 0)], Some(1))
        .unwrap();
    assert_eq!(text(&output.client_character_chunks[&client]), "     ");
    assert!(!output.cursor_is_visible(0, 0, None, client));
    let result = compose(50, chunk(&PLACEHOLDER.to_string(), 0), chunk(" ", 0));
    assert_eq!(text(&result), " ");
}

#[test]
fn reported_host_palette_resolves_colors_defaults_and_tint_before_blending() {
    use crate::panes::terminal_character::NamedColor;
    use yinz_types::{animation::blend, colors::eightbit_to_rgb};
    let client = ClientId::new(1);
    let clients = HashSet::from([client]);
    let host = HashMap::from([
        (1, "#283c50".into()),
        (2, "rgb:6464/6464/6464".into()),
        (4, "#14283c".into()),
        (9, "invalid".into()),
        (256, "#ffffff".into()),
    ]);
    for opacity in [0, 50] {
        for local in [None, Some(AnsiCode::RgbCode((60, 80, 100)))] {
            let mut output = Output::default();
            output.begin_translucent_composition(
                &clients,
                vec![(1, rect(0, 5), opacity)],
                PaletteColor::EightBit(1),
                PaletteColor::EightBit(4),
                PaletteColor::EightBit(2),
                &host,
            );
            let composition = output.compositions.get_mut(&client).unwrap();
            let mut under = chunk("abcde", 0);
            let mut changed = [None; 256];
            changed[1] = local;
            under.changed_colors = Some(changed);
            for (cell, color) in under.terminal_characters.iter_mut().zip([
                Some(AnsiCode::NamedColor(NamedColor::Red)),
                Some(AnsiCode::ColorIndex(1)),
                None,
                Some(AnsiCode::ColorIndex(9)),
                Some(AnsiCode::ColorIndex(10)),
            ]) {
                cell.styles = DEFAULT_STYLES
                    .foreground(color)
                    .background(Some(AnsiCode::NamedColor(NamedColor::Blue)))
                    .underline_color(color)
                    .into();
            }
            under.terminal_characters[2]
                .styles
                .update(|styles| styles.background = None);
            composition.add(&[under], 0);
            composition.add(&[chunk("     ", 0)], 1);
            let result = output.compositions.remove(&client).unwrap().finish();
            let expected_color = local.unwrap_or(AnsiCode::RgbCode((40, 60, 80)));
            let expected = [
                expected_color,
                expected_color,
                AnsiCode::RgbCode((40, 60, 80)),
                AnsiCode::RgbCode(eightbit_to_rgb(9)),
                AnsiCode::RgbCode(eightbit_to_rgb(10)),
            ];
            for (index, (cell, expected)) in result[0]
                .terminal_characters
                .iter()
                .zip(expected)
                .enumerate()
            {
                let AnsiCode::RgbCode(rgb) = expected else {
                    unreachable!()
                };
                let blended =
                    Some(blend(PaletteColor::Rgb(rgb), TINT, f32::from(opacity) / 100.0).into());
                assert_eq!(cell.styles.foreground, blended);
                if index != 2 {
                    assert_eq!(cell.styles.underline_color, blended);
                }
                assert_eq!(
                    cell.styles.background,
                    Some(blend(BG, TINT, f32::from(opacity) / 100.0).into())
                );
            }
        }
    }
}

#[test]
fn upper_opaque_wide_glyphs_survive_lower_translucent_edges() {
    for x in [0, 2] {
        let client = ClientId::new(1);
        let clients = HashSet::from([client]);
        let mut output = Output::default();
        output.add_clients(
            &clients,
            Rc::new(RefCell::new(LinkHandler::new())),
            Some(FloatingPanesStack {
                layers: vec![
                    PaneGeom {
                        x: 1,
                        y: 0,
                        cols: Dimension::fixed(2),
                        rows: Dimension::fixed(1),
                        ..Default::default()
                    },
                    PaneGeom {
                        x,
                        y: 0,
                        cols: Dimension::fixed(4),
                        rows: Dimension::fixed(1),
                        ..Default::default()
                    },
                ],
            }),
        );
        output.begin_translucent_composition(
            &clients,
            vec![(1, rect(1, 2), 50)],
            FG,
            BG,
            TINT,
            &HashMap::new(),
        );
        // Submit the upper mask first to exercise layer ordering.
        output
            .add_character_chunks_to_client(client, vec![chunk("界\u{301}ab", x)], Some(2))
            .unwrap();
        output
            .add_character_chunks_to_client(client, vec![chunk("abcdef", 0)], None)
            .unwrap();
        output
            .add_character_chunks_to_client(client, vec![chunk("  ", 1)], Some(1))
            .unwrap();
        output.finish_translucent_composition();
        let bytes = output.serialize().unwrap();
        let mut grid = crate::panes::Grid::new(
            1,
            8,
            Rc::new(RefCell::new(yinz_types::style::Palette::default())),
            Rc::new(RefCell::new(HashMap::new())),
            Rc::new(RefCell::new(LinkHandler::new())),
            Rc::new(RefCell::new(None)),
            Rc::new(RefCell::new(crate::panes::sixel::SixelImageStore::default())),
            Rc::new(RefCell::new(
                crate::panes::kitty_graphics::KittyImageStore::default(),
            )),
            yinz_types::style::Style::default(),
            false,
            true,
            true,
            true,
            false,
        );
        let mut parser = vte::Parser::new();
        parser.advance(&mut grid, bytes[&client].as_bytes());
        let screen = grid.serialize(None).unwrap();
        assert!(screen.contains("界ab"), "{screen:?}");
        assert!(bytes[&client].contains("界\u{301}ab"));
    }
}
