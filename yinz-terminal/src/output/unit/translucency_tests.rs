use super::*;
use std::collections::VecDeque;
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
fn cursor_cell_preserves_foreground_and_clips_underlying_wide_characters() {
    for (under, over, cursor, expected) in [
        ("abcde", "     ", 2, "ab de"),
        ("abcde", "  X  ", 2, "abXde"),
        ("a界de", "     ", 1, "a  de"),
        ("a界de", "     ", 2, "a  de"),
    ] {
        let mut composition =
            translucency::Composition::new(vec![(1, rect(0, 5), 50)], FG, BG, TINT, [None; 256]);
        composition.add(&[chunk(under, 0)], 0);
        composition.add(&[chunk(over, 0)], 1);
        composition.protect_cursor_cell(cursor, 0, 1);
        let cells = serialize_and_parse_row(composition.finish(), 5);
        assert_eq!(
            cells.iter().map(|c| c.character).collect::<String>(),
            expected
        );
        assert_eq!(cells[cursor].styles.background, Some(TINT.into()));
    }
}

#[test]
fn cursor_cell_protection_is_per_client_and_uses_cursor_layer_indices() {
    let clients = HashSet::from([ClientId::new(1), ClientId::new(2)]);
    let mut output = Output::default();
    output.add_clients(&clients, Rc::new(RefCell::new(LinkHandler::new())), None);
    output.begin_translucent_composition(
        &clients,
        vec![(1, rect(2, 5), 50)],
        FG,
        BG,
        TINT,
        &HashMap::new(),
    );
    for client in &clients {
        output
            .add_character_chunks_to_client(*client, vec![chunk("abcde", 2)], None)
            .unwrap();
        output
            .add_character_chunks_to_client(*client, vec![chunk("     ", 2)], Some(1))
            .unwrap();
    }
    output.protect_translucent_cursor_cell(ClientId::new(1), 4, 0, Some(0));
    // A tiled cursor and coordinates outside the overlay cannot punch holes in it.
    output.protect_translucent_cursor_cell(ClientId::new(2), 4, 0, None);
    output.protect_translucent_cursor_cell(ClientId::new(2), 0, 0, Some(0));
    output.protect_translucent_cursor_cell(ClientId::new(2), 8, 0, Some(0));
    output.protect_translucent_cursor_cell(ClientId::new(2), 4, 1, Some(0));
    output.finish_translucent_composition();
    assert_eq!(
        text(&output.client_character_chunks[&ClientId::new(1)]),
        "ab de"
    );
    assert_eq!(
        text(&output.client_character_chunks[&ClientId::new(2)]),
        "abcde"
    );
}

#[test]
fn screen_overlay_composes_fresh_content_and_notifications_in_paint_order() {
    let client = ClientId::new(1);
    for content in ["abcdefgh", "12345678"] {
        let mut output = Output::default();
        output.add_clients(
            &HashSet::from([client]),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        // A composed pane, then an opaque toast partially intersecting the palette.
        output
            .add_character_chunks_to_client(
                client,
                vec![chunk(content, 0), chunk("TOAST", 3)],
                None,
            )
            .unwrap();
        output
            .add_translucent_overlay(
                client,
                rect(0, 5),
                75,
                vec![chunk("  X  ", 0)],
                yinz_types::style::Palette {
                    fg: FG,
                    bg: BG,
                    ..Default::default()
                },
                TINT,
                &HashMap::new(),
                &mut TranslucentOverlayBacking::default(),
            )
            .unwrap();
        let chunks = output.client_character_chunks.remove(&client).unwrap();
        let cells = serialize_and_parse_row(chunks, 8);
        let visible: String = cells.iter().map(|c| c.character).collect();
        assert_eq!(visible, format!("{}XTOAST", &content[..2]));
        assert_eq!(
            cells[0].styles.foreground,
            Some(AnsiCode::RgbCode((125, 115, 105)))
        );
        assert_eq!(
            cells[0].styles.background,
            Some(AnsiCode::RgbCode((80, 85, 90)))
        );
        assert_eq!(cells[2].styles.background, Some(TINT.into()));
    }
}

#[test]
fn screen_overlay_reuses_untinted_backing_and_resets_it_on_display_clear() {
    let client = ClientId::new(1);
    let mut backing = TranslucentOverlayBacking::default();
    let palette = yinz_types::style::Palette {
        fg: FG,
        bg: BG,
        ..Default::default()
    };
    let mut original = None;
    for (under, over, cleared, expected) in [
        (vec![chunk("abcdefgh", 0)], " X ", false, "abcXefgh"),
        (vec![], "   ", false, "  cde"),
        (vec![chunk("Z", 3)], "   ", false, "  cZe"),
        (vec![], "   ", false, "  cZe"),
        (vec![], "   ", true, "     "),
    ] {
        let mut output = Output::default();
        output.add_clients(
            &HashSet::from([client]),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output
            .add_character_chunks_to_client(client, under, None)
            .unwrap();
        if cleared {
            output.mark_host_display_cleared_for_client(client);
        }
        output
            .add_translucent_overlay(
                client,
                rect(2, 3),
                75,
                vec![chunk(over, 2)],
                palette,
                TINT,
                &HashMap::new(),
                &mut backing,
            )
            .unwrap();
        assert!(!backing.prepare(rect(2, 3), 75, palette, TINT, &HashMap::new()));
        let cells =
            serialize_and_parse_row(output.client_character_chunks.remove(&client).unwrap(), 8);
        assert_eq!(
            cells.iter().map(|c| c.character).collect::<String>(),
            expected
        );
        if !cleared {
            assert_eq!(*original.get_or_insert(cells[2].clone()), cells[2]);
        }
    }
}

#[test]
fn screen_overlay_backing_invalidates_only_exposed_cells() {
    let mut backing = TranslucentOverlayBacking::default();
    let palette = yinz_types::style::Palette {
        fg: FG,
        bg: BG,
        ..Default::default()
    };
    backing.invalidate(&[rect(0, 10)]);
    backing.prepare(rect(2, 5), 75, palette, TINT, &HashMap::new());
    backing.update(&[chunk("abcde", 2)], false).unwrap();
    backing.invalidate(&[rect(0, 3), rect(4, 1), rect(6, 10)]);
    assert!(!backing.prepare(rect(2, 5), 75, palette, TINT, &HashMap::new()));
    let mut composition = backing.update(&[], false).unwrap();
    composition.add(&[chunk("     ", 2)], 1);
    let cells = serialize_and_parse_row(composition.finish(), 7);
    assert_eq!(
        cells.iter().map(|cell| cell.character).collect::<String>(),
        "   b d "
    );
}

#[test]
fn screen_overlay_backing_invalidates_for_geometry_and_colors() {
    let mut backing = TranslucentOverlayBacking::default();
    let mut palette = yinz_types::style::Palette {
        fg: FG,
        bg: BG,
        ..Default::default()
    };
    let mut codes = HashMap::new();
    for (area, tint) in [(rect(0, 5), TINT), (rect(2, 3), TINT), (rect(2, 3), BG)] {
        assert!(backing.prepare(area, 75, palette, tint, &codes));
        backing.update(&[chunk("abcdefgh", 0)], false).unwrap();
        assert!(!backing.prepare(area, 75, palette, tint, &codes));
    }
    palette.fg = TINT;
    assert!(backing.prepare(rect(2, 3), 75, palette, BG, &codes));
    backing.update(&[chunk("abcdefgh", 0)], false).unwrap();
    codes.insert(1, "rgb:00/ff/00".to_owned());
    assert!(backing.prepare(rect(2, 3), 75, palette, BG, &codes));
    // Newly exposed cells cannot retain old content before the caller's backing repaint.
    let composition = backing.update(&[], false).unwrap();
    assert!(composition.finish().is_empty());
}

#[test]
fn input_overlay_suppresses_all_underlying_cursors_for_its_client_only() {
    let mut output = Output::default();
    output.set_input_overlay_clients([ClientId::new(1)].into_iter());
    assert!(!output.cursor_is_visible(0, 0, None, ClientId::new(1)));
    assert!(output.cursor_is_visible(0, 0, None, ClientId::new(2)));
}

#[test]
fn screen_overlay_clips_both_graphics_protocols_and_composes_over_a_translucent_pane() {
    let client = ClientId::new(1);
    let mut output = Output::default();
    *output.character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 10,
        height: 20,
    });
    output.add_clients(
        &HashSet::from([client]),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    output.begin_translucent_composition(
        &HashSet::from([client]),
        vec![(1, rect(0, 5), 50)],
        FG,
        BG,
        TINT,
        &HashMap::new(),
    );
    output
        .add_character_chunks_to_client(client, vec![chunk("abcde", 0)], None)
        .unwrap();
    output
        .add_character_chunks_to_client(client, vec![chunk("     ", 0)], Some(1))
        .unwrap();
    output.finish_translucent_composition();
    output.add_sixel_image_chunks_to_client(
        client,
        vec![SixelImageChunk {
            cell_x: 0,
            cell_y: 0,
            sixel_image_pixel_x: 0,
            sixel_image_pixel_y: 0,
            sixel_image_pixel_width: 50,
            sixel_image_pixel_height: 20,
            sixel_image_id: 1,
        }],
        None,
    );
    let pane = PaneId::Terminal(yinz_types::internal_ids::TerminalId::new(1));
    output.add_kitty_image_chunks_to_client(
        client,
        pane,
        vec![KittyImageChunk {
            cell_x: 0,
            cell_y: 0,
            internal_image_id: 1,
            source_px_x: 0,
            source_px_y: 0,
            source_px_width: 50,
            source_px_height: 20,
            cell_offset_x: 0,
            cell_offset_y: 0,
            z_index: 0,
            dest_cells: (5, 1),
            variant_key: crate::panes::kitty_graphics::store::ScaledImageKey {
                source: crate::panes::sixel::PixelRect::new(0, 0, 20, 50),
                size: (50, 20),
            },
            scaled_px: None,
            placement_uid: 1,
        }],
        None,
    );
    output
        .add_translucent_overlay(
            client,
            rect(1, 3),
            75,
            vec![chunk("   ", 1)],
            yinz_types::style::Palette {
                fg: FG,
                bg: BG,
                ..Default::default()
            },
            TINT,
            &HashMap::new(),
            &mut TranslucentOverlayBacking::default(),
        )
        .unwrap();
    let cells = serialize_and_parse_row(output.client_character_chunks.remove(&client).unwrap(), 5);
    assert_eq!(
        cells.iter().map(|c| c.character).collect::<String>(),
        "abcde"
    );
    assert_eq!(
        cells[1].styles.foreground,
        Some(AnsiCode::RgbCode((113, 108, 103)))
    );
    // The Sixel clipper also retains empty top/bottom crops; they paint no pixels.
    let visible_sixel: Vec<_> = output.sixel_chunks[&client]
        .iter()
        .filter(|chunk| chunk.sixel_image_pixel_width > 0 && chunk.sixel_image_pixel_height > 0)
        .collect();
    for chunk in &visible_sixel {
        assert!(chunk.cell_x == 0 || chunk.cell_x >= 4);
        assert!(chunk.sixel_image_pixel_width <= 10);
    }
    assert_eq!(visible_sixel.len(), 2);
    for chunk in &output.client_kitty_chunks[&client][&pane] {
        assert!(chunk.cell_x == 0 || chunk.cell_x >= 4);
        assert!(chunk.source_px_width <= 10);
    }
    assert_eq!(output.client_kitty_chunks[&client][&pane].len(), 2);
}

fn serialize_and_parse_row(
    chunks: Vec<CharacterChunk>,
    cols: usize,
) -> VecDeque<TerminalCharacter> {
    let mut bytes = String::new();
    serialize_chunks(&mut bytes, chunks, None, None, None, true, true, None, None).unwrap();
    parse_row(bytes.as_bytes(), cols)
        .viewport
        .pop_front()
        .unwrap()
        .columns
}

fn parse_row(bytes: &[u8], cols: usize) -> crate::panes::Grid {
    let mut grid = crate::panes::Grid::new(
        1,
        cols,
        Rc::new(RefCell::new(yinz_types::style::Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        yinz_types::style::Style::default(),
        false,
        true,
        true,
        true,
        false,
    );
    vte::Parser::new().advance(&mut grid, bytes);
    grid
}

#[test]
fn serialization_preserves_all_bold_and_dim_transitions() {
    use crate::panes::terminal_character::RESET_STYLES;

    let styles = [
        RESET_STYLES,
        RESET_STYLES.bold(Some(AnsiCode::On)),
        RESET_STYLES.dim(Some(AnsiCode::On)),
        RESET_STYLES
            .bold(Some(AnsiCode::On))
            .dim(Some(AnsiCode::On)),
    ];
    for from in styles {
        for to in styles {
            // Repeat the destination to check the tracked state after the transition too.
            let expected = [from, to, to];
            let characters = expected
                .iter()
                .map(|styles| TerminalCharacter::new_styled('x', (*styles).into()))
                .collect();
            let cells = serialize_and_parse_row(vec![CharacterChunk::new(characters, 0, 0)], 3);
            assert_eq!(cells.len(), expected.len());
            for (cell, expected) in cells.iter().zip(expected) {
                assert_eq!(cell.styles.bold, expected.bold, "{from:?} -> {to:?}");
                assert_eq!(cell.styles.dim, expected.dim, "{from:?} -> {to:?}");
            }
        }
    }
}

#[test]
fn shared_styles_keep_selection_boundaries_after_wide_and_combining_characters() {
    use crate::panes::terminal_character::RESET_STYLES;
    use yinz_types::position::Position;

    let pen: RcCharacterStyles = RESET_STYLES.foreground(Some(FG.into())).into();
    let mut under = CharacterChunk::new(
        "a界b\u{301}cd"
            .chars()
            .map(|c| TerminalCharacter::new_styled(c, pen.clone()))
            .collect(),
        0,
        0,
    );
    let mut selection = Selection::default();
    selection.set_start_and_end_positions(Position::new(0, 3), Position::new(0, 5));
    under.add_selection_and_colors(
        HighlightSelection {
            selection,
            fg: Some(TINT.into()),
            bg: Some(BG.into()),
            bold: true,
            italic: true,
            underline: true,
            layer: HighlightLayer::Hint,
        },
        0,
        0,
    );
    let mut composition =
        translucency::Composition::new(vec![(1, rect(0, 6), 0)], FG, BG, TINT, [None; 256]);
    composition.add(&[under.clone()], 0);
    composition.add(&[chunk("      ", 0)], 1);
    for chunks in [vec![under], composition.finish()] {
        let cells = serialize_and_parse_row(chunks, 6);
        assert_eq!(
            cells.iter().map(|c| c.character).collect::<String>(),
            "a界bcd"
        );
        for cell in cells {
            let selected = matches!(cell.character, 'b' | 'c');
            assert_eq!(
                cell.styles.foreground,
                Some(if selected { TINT } else { FG }.into())
            );
            let attribute = Some(if selected {
                AnsiCode::On
            } else {
                AnsiCode::Reset
            });
            assert_eq!(cell.styles.bold, attribute);
            assert_eq!(cell.styles.italic, attribute);
            assert_eq!(
                cell.styles.underline,
                Some(if selected {
                    AnsiCode::Underline(None)
                } else {
                    AnsiCode::Reset
                })
            );
        }
    }
}

#[test]
fn resolved_style_runs_use_each_chunks_palette_and_pane_defaults() {
    let pen: RcCharacterStyles = DEFAULT_STYLES
        .foreground(Some(AnsiCode::ColorIndex(1)))
        .underline_color(Some(AnsiCode::ColorIndex(1)))
        .into();
    let mut composition =
        translucency::Composition::new(vec![(1, rect(0, 6), 50)], FG, BG, TINT, [None; 256]);
    for (x, foreground, background) in [(0, FG, BG), (3, BG, FG)] {
        let mut under = CharacterChunk::new(
            vec![TerminalCharacter::new_styled('x', pen.clone()); 3],
            x,
            0,
        );
        let mut changed = [None; 256];
        changed[1] = Some(foreground.into());
        under.changed_colors = Some(changed);
        under.pane_default_bg = Some(background.into());
        // Separate additions also exercise retaining cells from earlier chunks in this layer.
        composition.add(&[under], 0);
    }
    composition.add(&[chunk("      ", 0)], 1);
    let composed = composition.finish();
    assert_eq!(text(&composed), "xxxxxx");
    for (index, cell) in composed[0].terminal_characters.iter().enumerate() {
        let (fg, bg) = if index < 3 { (FG, BG) } else { (BG, FG) };
        let blended = |color| Some(yinz_types::animation::blend(color, TINT, 0.5).into());
        assert_eq!(cell.styles.foreground, blended(fg));
        assert_eq!(cell.styles.underline_color, blended(fg));
        assert_eq!(cell.styles.background, blended(bg));
    }
}

#[test]
fn blended_style_runs_preserve_links_and_underline_colors_across_opacity_layers() {
    use crate::panes::terminal_character::{AnsiStyledUnderline, LinkAnchor, RESET_STYLES};
    use yinz_types::animation::blend;

    let base = RESET_STYLES
        .foreground(Some(FG.into()))
        .background(Some(BG.into()))
        .underline(Some(AnsiCode::Underline(Some(
            AnsiStyledUnderline::Undercurl,
        ))));
    let mut composition = translucency::Composition::new(
        vec![(1, rect(0, 6), 50), (2, rect(2, 2), 75)],
        FG,
        BG,
        TINT,
        [None; 256],
    );
    let source: Vec<_> = (0..6)
        .map(|index| {
            let link = if index < 2 {
                LinkAnchor::Start(0)
            } else {
                LinkAnchor::End
            };
            let underline = if index < 4 { FG } else { BG };
            TerminalCharacter::new_styled(
                'x',
                base.link_anchor(Some(link))
                    .underline_color(Some(underline.into()))
                    .into(),
            )
        })
        .collect();
    composition.add(&[CharacterChunk::new(source.clone(), 0, 0)], 0);
    composition.add(&[chunk("      ", 0)], 1);
    composition.add(&[chunk("  ", 2)], 2);
    let composed = composition.finish();
    assert_eq!(text(&composed), "xxxxxx");
    for (index, (cell, original)) in composed[0]
        .terminal_characters
        .iter()
        .zip(source)
        .enumerate()
    {
        let tint = |color| {
            let color = blend(color, TINT, 0.5);
            Some(
                if (2..4).contains(&index) {
                    blend(color, TINT, 0.75)
                } else {
                    color
                }
                .into(),
            )
        };
        assert_eq!(cell.styles.foreground, tint(FG));
        assert_eq!(cell.styles.background, tint(BG));
        assert_eq!(
            cell.styles.underline_color,
            tint(if index < 4 { FG } else { BG })
        );
        assert_eq!(cell.styles.underline, original.styles.underline);
        assert_eq!(cell.styles.link_anchor, original.styles.link_anchor);
    }
}

#[test]
fn translucent_lettering_preserves_bold_dim_and_foreground_after_serialization() {
    use crate::panes::terminal_character::RESET_STYLES;

    let foreground = Some(AnsiCode::RgbCode((80, 120, 160)));
    for opacity in [0, 50, 75] {
        for bold in [AnsiCode::Reset, AnsiCode::On] {
            let lettering = RESET_STYLES
                .foreground(foreground)
                .bold(Some(bold))
                .dim(Some(AnsiCode::On));
            let underlying = RESET_STYLES
                .bold(Some(AnsiCode::On))
                .dim(Some(AnsiCode::On));
            let mut under = chunk("abcde", 0);
            for cell in &mut under.terminal_characters {
                cell.styles = underlying.into();
            }
            let mut over = chunk("H C S", 0);
            for cell in &mut over.terminal_characters {
                cell.styles = lettering.into();
            }
            let cells = serialize_and_parse_row(compose(opacity, under, over), 5);
            assert_eq!(
                cells.iter().map(|c| c.character).collect::<String>(),
                "HbCdS"
            );
            for index in [0, 2, 4] {
                let styles = &cells[index].styles;
                assert_eq!(styles.foreground, foreground, "opacity {opacity}");
                assert_eq!(styles.bold, Some(bold), "opacity {opacity}");
                assert_eq!(styles.dim, Some(AnsiCode::On), "opacity {opacity}");
            }
        }
    }
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
fn disjoint_rectangles_keep_row_order_and_gaps_with_out_of_order_chunks() {
    let mut composition = translucency::Composition::new(
        vec![
            (
                1,
                CellRect {
                    x: 2,
                    y: 1,
                    cols: 3,
                    rows: 2,
                },
                50,
            ),
            (
                2,
                CellRect {
                    x: 7,
                    y: 2,
                    cols: 2,
                    rows: 1,
                },
                0,
            ),
        ],
        FG,
        BG,
        TINT,
        [None; 256],
    );
    for (layer, x, y, content) in [
        (2, 7, 2, "  "),
        (0, 7, 2, "hi"),
        (0, 0, 2, "abcde"),
        (1, 2, 2, "   "),
        (1, 2, 1, "   "),
        (0, 0, 1, "01234"),
    ] {
        let mut row = chunk(content, x);
        row.y = y;
        composition.add(&[row], layer);
    }
    let result = composition.finish();
    assert_eq!(
        result
            .iter()
            .map(|c| (c.x, c.y, text(std::slice::from_ref(c))))
            .collect::<Vec<_>>(),
        vec![
            (2, 1, "234".into()),
            (2, 2, "cde".into()),
            (7, 2, "hi".into())
        ]
    );
    assert_eq!(
        result[0].terminal_characters[0].styles.foreground,
        Some(AnsiCode::RgbCode((150, 130, 110)))
    );
    assert_eq!(
        result[2].terminal_characters[0].styles.foreground,
        Some(FG.into())
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

#[test]
fn theme_backgrounds_allow_opacity_but_explicit_pane_colors_stay_solid() {
    let mut over = chunk("     ", 0);
    over.add_pane_defaults(
        Some(AnsiCode::RgbCode((1, 2, 3))),
        Some(AnsiCode::RgbCode((4, 5, 6))),
    );
    over.theme_background = true;
    let result = compose(50, chunk("abcde", 0), over.clone());
    assert_eq!(text(&result), "abcde");
    assert_eq!(
        result[0].terminal_characters[0].styles.background,
        Some(AnsiCode::RgbCode((60, 70, 80)))
    );
    over.theme_background = false;
    let result = compose(50, chunk("abcde", 0), over);
    assert_eq!(text(&result), "     ");
    assert_eq!(
        result[0].terminal_characters[0].styles.background,
        Some(AnsiCode::RgbCode((4, 5, 6)))
    );
}

#[test]
fn serialization_remaps_sgr_colors_with_theme_and_application_palettes() {
    use yinz_types::style::{Style, TerminalTheme};

    let theme = TerminalTheme {
        foreground: (1, 2, 3),
        background: (4, 5, 6),
        ansi: std::array::from_fn(|i| (i as u8, 40, 80)),
    };
    for named in [false, true] {
        let mut input = String::new();
        for i in 0..16 {
            if named {
                let fg = if i < 8 { 30 + i } else { 90 + i - 8 };
                let bg = fg + 10;
                input.push_str(&format!("\x1b[{fg};{bg};58;5;{i}mX"));
            } else {
                input.push_str(&format!("\x1b[38;5;{i};48;5;{i};58;5;{i}mX"));
            }
        }
        input.push_str("\x1b[38;5;200;48;5;200;58;5;200mE");
        input.push_str("\x1b[38;2;11;22;33;48;2;44;55;66;58;2;77;88;99mT\x1b[0mD");
        for themed in [false, true] {
            for overridden in [false, true] {
                let mut grid = parse_row(input.as_bytes(), 19);
                let original = grid.viewport[0].columns.clone();
                if themed {
                    grid.update_terminal_theme(Some(theme));
                }
                if overridden {
                    vte::Parser::new().advance(&mut grid, b"\x1b]4;1;#123456;200;#abcdef\x07");
                }
                let (chunks, _, _, _) = grid.render(0, 0, &Style::default()).unwrap().unwrap();
                let cells = serialize_and_parse_row(chunks, 19);
                assert_eq!(grid.viewport[0].columns, original);
                for (i, cell) in cells.iter().take(17).enumerate() {
                    let replacement = if overridden && i == 1 {
                        Some(AnsiCode::RgbCode((0x12, 0x34, 0x56)))
                    } else if overridden && i == 16 {
                        Some(AnsiCode::RgbCode((0xab, 0xcd, 0xef)))
                    } else if themed && i < 16 {
                        Some(AnsiCode::RgbCode(theme.ansi[i]))
                    } else {
                        None
                    };
                    assert_eq!(
                        cell.styles.foreground,
                        replacement.or(original[i].styles.foreground)
                    );
                    assert_eq!(
                        cell.styles.background,
                        replacement.or(original[i].styles.background)
                    );
                    assert_eq!(
                        cell.styles.underline_color,
                        replacement.or(original[i].styles.underline_color)
                    );
                }
                assert_eq!(cells[17].styles.foreground, original[17].styles.foreground);
                assert_eq!(cells[17].styles.background, original[17].styles.background);
                assert_eq!(
                    cells[17].styles.underline_color,
                    original[17].styles.underline_color
                );
                assert_eq!(
                    cells[18].styles.foreground,
                    Some(if themed {
                        AnsiCode::RgbCode(theme.foreground)
                    } else {
                        AnsiCode::Reset
                    })
                );
                assert_eq!(
                    cells[18].styles.background,
                    Some(if themed {
                        AnsiCode::RgbCode(theme.background)
                    } else {
                        AnsiCode::Reset
                    })
                );
                assert_eq!(cells[18].styles.underline_color, Some(AnsiCode::Reset));
            }
        }
    }
}

#[test]
fn semantic_backgrounds_remain_translucent_and_selection_wins() {
    use yinz_types::agent::theme::*;
    let snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "codex".into(),
        provider_version: "fixture".into(),
        theme: "fixture".into(),
        provenance: vec![],
        availability: Availability::Available,
        collisions: vec![],
        definitions: vec![Definition {
            token: "background".into(),
            channel: Channel::Background,
            color: SourceColor::Rgb(1, 2, 3),
            pair: None,
            target: Some(Role::Background.into()),
        }],
    };
    let mut over = chunk("  X  ", 0);
    over.agent_theme = Some(Rc::new(crate::agent_theme::SourcePalette::new(
        Some(snapshot),
        yinz_types::style::Style::default(),
    )));
    for cell in &mut over.terminal_characters {
        cell.styles = DEFAULT_STYLES
            .background(Some(AnsiCode::RgbCode((1, 2, 3))))
            .into();
    }
    let mapped = compose(50, chunk("abcde", 0), over.clone());
    let expected = compose(50, chunk("abcde", 0), chunk("  X  ", 0));
    assert_eq!(
        serialize_and_parse_row(mapped, 5),
        serialize_and_parse_row(expected, 5)
    );

    let mut selection = Selection::default();
    selection.set_start_and_end_positions(
        yinz_types::position::Position::new(0, 0),
        yinz_types::position::Position::new(0, 1),
    );
    over.add_selection_and_colors(
        HighlightSelection {
            selection,
            bg: Some(AnsiCode::RgbCode((4, 5, 6))),
            fg: None,
            bold: false,
            italic: false,
            underline: false,
            layer: HighlightLayer::Hint,
        },
        0,
        0,
    );
    let selected = serialize_and_parse_row(compose(0, chunk("abcde", 0), over), 5);
    assert_eq!(selected[0].character, ' ');
    assert_eq!(
        selected[0].styles.background,
        Some(AnsiCode::RgbCode((4, 5, 6)))
    );
}
