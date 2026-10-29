use super::*;
use yinz_runtime::channels;
use yinz_tile::ui_components::{image_row, place_image_sequence, png_transmit_sequence};
use yinz_types::pane_size::Dimension;
use yinz_types::ui_components::UI_COMPONENT_DCS_PREFIX;

pub(super) fn test_plugin_pane(rows: usize, cols: usize, content_offset: Offset) -> PluginPane {
    test_plugin_pane_with_cell_size(rows, cols, content_offset, None)
}

pub(super) fn frame_params() -> FrameParams {
    FrameParams {
        focused_client: Some(ClientId::new(1)),
        is_main_client: true,
        other_focused_clients: vec![],
        style: Style::default(),
        color: None,
        other_cursors_exist_in_session: false,
        pane_is_stacked_over: false,
        pane_is_stacked_under: false,
        pane_is_stacked: false,
        should_draw_pane_frames: true,
        pane_is_floating: false,
        show_pinned_indication: false,
        show_scroll_indication: true,
        content_offset: Offset::frame(1),
        mouse_is_hovering_over_pane: false,
        pane_is_selectable: true,
        show_help_text: false,
        highlight_tooltip: None,
        omit_title: false,
        frame_geom_override: None,
        stack_list_entry: None,
        blank_title: false,
        same_worktree: false,
        mouse_scroll_resize: false,
        dimmed: false,
        guest_choice_indicator: None,
        opening_ripple: None,
    }
}

fn rendered_text(chunks: &[CharacterChunk]) -> String {
    chunks
        .iter()
        .flat_map(|chunk| chunk.terminal_characters.iter().map(|c| c.character))
        .collect()
}

#[test]
fn initial_contents_seed_each_plugin_client_until_its_first_frame() {
    let mut pane = test_plugin_pane(5, 40, Offset::frame(1));
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    for client in [first, second] {
        pane.add_client(client);
    }
    let pane: &mut dyn Pane = &mut pane;
    pane.set_initial_contents("restored λ\r\nsecond row");
    for client in [first, second] {
        assert_eq!(
            pane.dump_screen(false, Some(client)).trim_end(),
            "restored λ\nsecond row"
        );
    }
    pane.handle_plugin_bytes(first, b"live frame".to_vec());
    assert_eq!(
        pane.dump_screen(false, Some(first)).trim_end(),
        "live frame"
    );
    assert!(pane.dump_screen(false, Some(second)).contains("restored λ"));
}

#[test]
fn pane_tabs_views_preserve_each_clients_highlight_and_reveal() {
    let mut pane = test_plugin_pane(5, 40, Offset::frame(1));
    let first = ClientId::new(1);
    let second = ClientId::new(2);
    let first_view = crate::pane_tabs::PaneTabsView {
        highlight: 1,
        reveal: Some(2),
    };
    let second_view = crate::pane_tabs::PaneTabsView {
        highlight: 3,
        reveal: None,
    };
    let pane: &mut dyn Pane = &mut pane;
    pane.set_pane_tabs_views(HashMap::from([(first, first_view), (second, second_view)]));
    assert_eq!(pane.pane_tabs_view(first), first_view);
    assert_eq!(pane.pane_tabs_view(second), second_view);
    assert_eq!(pane.pane_tabs_view(ClientId::new(3)), Default::default());
    pane.remove_client(first);
    assert_eq!(pane.pane_tabs_view(first), Default::default());
    assert_eq!(pane.pane_tabs_view(second), second_view);
}

#[test]
fn ending_a_selection_marks_the_pane_for_rendering() {
    // `start_selection` and `update_selection` set the flag; `end_selection` did
    // not, so a double-click word selection stayed invisible until an unrelated
    // event repainted the pane.
    let (mut pane, _plugin_rx) =
        test_plugin_pane_and_plugin_receiver(5, 40, Offset::frame(1), None);
    let client_id = ClientId::new(1);
    pane.handle_plugin_bytes(client_id, b"some text to select".to_vec());
    pane.set_mouse_selection_support(true);
    pane.start_selection(&Position::new(0, 0), client_id);
    pane.set_should_render(false);

    pane.end_selection(&Position::new(0, 4), client_id);

    assert!(pane.should_render());
}

/// A pane with a finished mouse selection over its first word, as a drag-to-copy leaves it.
fn pane_with_a_selection() -> (
    PluginPane,
    channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
) {
    let (mut pane, plugin_rx) = test_plugin_pane_and_plugin_receiver(5, 40, Offset::frame(1), None);
    let client_id = ClientId::new(1);
    pane.handle_plugin_bytes(client_id, b"some text to select".to_vec());
    pane.set_mouse_selection_support(true);
    pane.start_selection(&Position::new(0, 0), client_id);
    pane.update_selection(&Position::new(0, 4), client_id);
    pane.end_selection(&Position::new(0, 4), client_id);
    assert!(pane.grids[&client_id].has_selection());
    (pane, plugin_rx)
}

/// The first key after a drag-to-copy used to be spent clearing the highlight, so the `j`,
/// arrow or Tab that followed every copy did nothing.
#[test]
fn a_key_after_a_selection_clears_it_and_still_reaches_the_plugin() {
    let (mut pane, _plugin_rx) = pane_with_a_selection();
    let client_id = ClientId::new(1);
    let j = KeyWithModifier::new(BareKey::Char('j'));

    let adjusted =
        pane.adjust_input_to_terminal(&Some(j.clone()), b"j".to_vec(), false, Some(client_id));

    assert!(
        matches!(&adjusted, Some(AdjustedInput::WriteKeyToPlugin(key)) if *key == j),
        "{adjusted:?}"
    );
    assert!(!pane.grids[&client_id].has_selection());
}

/// Esc is how you let go of a selection, so it stops at clearing it: passed on as well, it
/// would close whatever the plugin has open.
#[test]
fn esc_after_a_selection_only_clears_it() {
    for key in [Some(KeyWithModifier::new(BareKey::Esc)), None] {
        let (mut pane, _plugin_rx) = pane_with_a_selection();
        let client_id = ClientId::new(1);

        let adjusted = pane.adjust_input_to_terminal(&key, vec![27], false, Some(client_id));

        assert!(adjusted.is_none(), "{key:?}: {adjusted:?}");
        assert!(!pane.grids[&client_id].has_selection());
    }
}

/// A paste after a selection used to lose its bracketed-paste opener to the clearing, so the
/// text arrived as keystrokes - a newline as Enter, letters as commands.
#[test]
fn a_paste_after_a_selection_still_arrives_as_a_paste() {
    let (mut pane, _plugin_rx) = pane_with_a_selection();
    let client_id = ClientId::new(1);

    let adjusted = pane.adjust_input_to_terminal(
        &None,
        BRACKETED_PASTE_BEGIN.to_vec(),
        false,
        Some(client_id),
    );

    assert!(adjusted.is_none(), "{adjusted:?}");
    assert_eq!(pane.text_being_pasted, Some(vec![]));
    assert!(!pane.grids[&client_id].has_selection());
}

#[test]
fn a_plugin_pane_frame_never_shows_a_scroll_indicator() {
    // The host grid keeps a scrollback for a plugin that renders more rows than fit,
    // but the host cannot scroll a plugin pane - `scroll_up` only forwards a mouse
    // event to the plugin - so the indicator sat at `SCROLL: 0/N` for good.
    let mut pane = test_plugin_pane(5, 60, Offset::frame(1));
    let client_id = ClientId::new(1);
    let many_rows: String = (0..30).map(|i| format!("row {i}\r\n")).collect();
    pane.handle_plugin_bytes(client_id, many_rows.into_bytes());
    let scrollback = pane.grids[&client_id].scrollback_position_and_length().1;
    assert!(
        scrollback > 0,
        "precondition: the host grid holds scrollback"
    );

    let (frame, _) = pane
        .render_frame(client_id, frame_params(), InputMode::Normal)
        .unwrap()
        .expect("a framed pane renders a frame");

    assert!(
        !rendered_text(&frame).contains("SCROLL"),
        "{}",
        rendered_text(&frame)
    );
}

#[test]
fn subtitle_resizes_plugin_content_and_omits_small_or_borderless_panes() {
    for offset in [
        Offset::frame(1),
        Offset::shift_right_top_and_bottom(0, 1, 0),
    ] {
        let mut pane = test_plugin_pane(6, 20, offset);
        let before = pane.get_content_rows();
        pane.set_subtitle(Some("review\n · tests\t · running".into()));
        assert_eq!(pane.subtitle(), Some("review · tests · running"));
        assert_eq!(pane.get_content_rows(), before - 1);
        assert_eq!(pane.get_content_y(), offset.top + 1);
        let mut geom = pane.current_geom();
        geom.rows = Dimension::fixed(offset.top + offset.bottom + 1);
        pane.set_geom(geom);
        assert_eq!(pane.get_content_rows(), 1);
        assert_eq!(pane.subtitle_rows(), 0);
        geom.rows = Dimension::fixed(6);
        geom.cols = Dimension::fixed(offset.left + offset.right);
        pane.set_geom(geom);
        assert_eq!(pane.subtitle_rows(), 0);
        geom.cols = Dimension::fixed(20);
        pane.set_geom(geom);
        assert_eq!(pane.subtitle_rows(), 1);
        pane.set_subtitle(None);
        assert_eq!(pane.get_content_rows(), before);
        pane.set_subtitle(Some("again".into()));
        pane.set_borderless(true);
        assert_eq!(pane.subtitle_rows(), 0);
        assert_eq!(pane.get_content_rows(), 6);
    }
}

/// Placeholder placements are only resolved once the character cell size is known
/// (`Grid::kitty_refresh_placeholder_placements`), so an image test has to supply one.
fn test_plugin_pane_with_cell_size(
    rows: usize,
    cols: usize,
    content_offset: Offset,
    cell_size: Option<SizeInPixels>,
) -> PluginPane {
    test_plugin_pane_and_plugin_receiver(rows, cols, content_offset, cell_size).0
}

/// Also hands back the plugin thread's receiver, for a test that drives a path which
/// sends to it (the mouse handlers `unwrap` that send).
pub(super) fn test_plugin_pane_and_plugin_receiver(
    rows: usize,
    cols: usize,
    content_offset: Offset,
    cell_size: Option<SizeInPixels>,
) -> (
    PluginPane,
    channels::Receiver<(PluginInstruction, yinz_types::diagnostics::ErrorContext)>,
) {
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let geom = PaneGeom {
        rows: Dimension::fixed(rows),
        cols: Dimension::fixed(cols),
        ..Default::default()
    };
    let mut pane = PluginPane::new(
        PluginId::new(1),
        geom,
        SenderWithContext::new(plugin_tx),
        "test".to_owned(),
        "test".to_owned(),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(cell_size)),
        vec![],
        Style::default(),
        None,
        false,
        false,
        false,
    );
    pane.set_content_offset(content_offset);
    (pane, plugin_rx)
}

// regression test: a content offset (the border/title reservation a framed pane applies)
// used to make `cursor_coordinates` reject any cursor on the
// last content row/column, because it bounds-checked the offset-shifted coordinates
// against the un-shifted content extents.
#[test]
fn cursor_coordinates_are_shown_on_the_last_content_row_with_a_top_offset() {
    let mut pane = test_plugin_pane(
        10,
        10,
        Offset {
            top: 1,
            bottom: 0,
            left: 0,
            right: 0,
        },
    );
    let last_content_row = pane.get_content_rows() - 1;
    pane.show_cursor(ClientId::new(1), Some((0, last_content_row)));
    assert_eq!(
        pane.cursor_coordinates(Some(ClientId::new(1))),
        Some((0, last_content_row + 1, true))
    );
}

#[test]
fn cursor_coordinates_are_shown_on_the_last_content_column_with_a_left_offset() {
    let mut pane = test_plugin_pane(
        10,
        10,
        Offset {
            top: 0,
            bottom: 0,
            left: 1,
            right: 0,
        },
    );
    let last_content_column = pane.get_content_columns() - 1;
    pane.show_cursor(ClientId::new(1), Some((last_content_column, 0)));
    assert_eq!(
        pane.cursor_coordinates(Some(ClientId::new(1))),
        Some((last_content_column + 1, 0, true))
    );
}

#[test]
fn a_keyboard_selection_moves_the_cursor_to_the_end_being_moved() {
    let mut pane = test_plugin_pane(
        10,
        10,
        Offset {
            top: 1,
            bottom: 0,
            left: 1,
            right: 0,
        },
    );
    pane.set_mouse_selection_support(true);
    pane.handle_plugin_bytes(ClientId::new(1), b"abcdef".to_vec());
    pane.show_cursor(ClientId::new(1), Some((0, 0)));

    pane.start_text_selection(ClientId::new(1));
    pane.move_text_selection(Direction::Right, ClientId::new(1));
    pane.move_text_selection(Direction::Right, ClientId::new(1));
    assert_eq!(
        pane.cursor_coordinates(Some(ClientId::new(1))),
        Some((3, 1, true)),
        "the cursor follows the selection's live end, offset by the frame"
    );

    pane.swap_text_selection_ends(ClientId::new(1));
    assert_eq!(
        pane.cursor_coordinates(Some(ClientId::new(1))),
        Some((1, 1, true)),
        "swapping the ends is visible on a plugin pane too"
    );

    pane.finish_text_selection(ClientId::new(1));
    assert_eq!(
        pane.cursor_coordinates(Some(ClientId::new(1))),
        Some((1, 1, true)),
        "the plugin's own cursor is back once the selection is finished"
    );
}

fn ytext(x: usize, y: usize, width: usize, text: &str) -> String {
    let bytes = text
        .as_bytes()
        .iter()
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("{UI_COMPONENT_DCS_PREFIX}text;{x}/{y}/{width}/;{bytes}\u{1b}\\")
}

// regression test: the host pads a component out to the width it is given, so a
// component asked for at an indented `x` with the pane's full width used to run past
// the right edge and wrap. On the bottom row - where a dialog sized to exactly the
// rows it asked for draws its help line - the wrap scrolled the whole frame up by
// one line, and every widget then sat one row above the rectangle the plugin
// hit-tests clicks against.
#[test]
fn a_component_wider_than_the_room_to_its_right_does_not_scroll_the_pane() {
    let mut pane = test_plugin_pane(5, 20, Offset::default());
    let rows = pane.get_content_rows();
    let columns = pane.get_content_columns();
    let frame = format!(
        "{}{}",
        ytext(0, 0, columns, "top"),
        ytext(5, rows - 1, columns, "bottom"),
    );
    pane.handle_plugin_bytes(ClientId::new(1), frame.into_bytes());

    // `Grid`'s `Debug` prints one viewport row per line, each behind a `00 (C): `
    // index prefix.
    let rendered = format!(
        "{:?}",
        pane.grids
            .get(&ClientId::new(1))
            .expect("the grid was created")
    );
    let row_content = |line: &str| {
        line.split_once("): ")
            .map(|(_, content)| content.trim_end().to_owned())
            .unwrap_or_default()
    };
    let lines: Vec<String> = rendered.lines().map(row_content).collect();
    assert_eq!(lines.first().map(String::as_str), Some("top"), "{rendered}");
    assert_eq!(
        lines.last().map(String::as_str),
        Some("     bottom"),
        "{rendered}"
    );
}

#[test]
fn cursor_coordinates_are_none_past_the_content_area() {
    let mut pane = test_plugin_pane(
        10,
        10,
        Offset {
            top: 1,
            bottom: 0,
            left: 0,
            right: 0,
        },
    );
    let content_rows = pane.get_content_rows();
    pane.show_cursor(ClientId::new(1), Some((0, content_rows)));
    assert_eq!(pane.cursor_coordinates(Some(ClientId::new(1))), None);
}

/// The frame a plugin drawing an image emits, built with the very helpers a plugin calls -
/// A second, hand-written copy of the
/// escape sequences here would pass while the two halves disagreed.
pub(super) fn image_frame(id: u32, cols: usize, png: &[u8]) -> Vec<u8> {
    let mut frame = png_transmit_sequence(id, png);
    frame.push_str(&place_image_sequence(id, cols, 1));
    frame.push_str(&image_row(id, 0, cols));
    frame.into_bytes()
}

/// A PNG wide enough that its base64 does not fit in one command, so `transmit_png` has to
/// chunk it - which is the shape agent's wand frames arrive in.
fn chunky_png() -> Vec<u8> {
    let (width, height) = (64u32, 64u32);
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        // Noise rather than a flat colour: a flat image compresses to a single chunk and the
        // chunking test would silently stop testing chunking.
        let pixels: Vec<u8> = (0..(width * height * 4))
            .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        writer.write_image_data(&pixels).expect("png data");
    }
    png
}

/// A 2x2 PNG, small enough to go over in one command.
pub(super) fn tiny_png() -> Vec<u8> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&[0; 16]).expect("png data");
    }
    png
}

pub(super) fn kitty_chunks(pane: &mut PluginPane, client_id: ClientId) -> Vec<KittyImageChunk> {
    pane.render(Some(client_id))
        .expect("render failed")
        .map(|(_, _, _, kitty)| kitty)
        .unwrap_or_default()
}

#[test]
fn a_plugin_can_place_an_image_with_unicode_placeholders() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 8,
            height: 16,
        }),
    );
    pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
    let chunks = kitty_chunks(&mut pane, ClientId::new(1));
    // A contiguous placeholder run resolves to one chunk covering the cells it spans, not one
    // chunk per cell.
    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert_eq!(chunks[0].dest_cells, (2, 1));
    assert_eq!((chunks[0].cell_x, chunks[0].cell_y), (0, 0));
}

#[test]
fn plugin_cleanup_releases_only_its_own_images_and_all_caches() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
    );
    let store = pane.kitty_image_store.clone();
    let mut surviving = Vec::new();
    for client in [ClientId::new(1), ClientId::new(2)] {
        pane.handle_plugin_bytes(client, image_frame(42, 2, &tiny_png()));
        let chunks = kitty_chunks(&mut pane, client);
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0];
        store
            .borrow_mut()
            .zlib_base64_for(chunk.internal_image_id, Some(chunk.variant_key))
            .unwrap()
            .unwrap();
        store
            .borrow_mut()
            .write_frame(chunk.internal_image_id, None, vec![7; 16], 40)
            .unwrap();
        if client == ClientId::new(2) {
            surviving.push(chunk.internal_image_id);
        }
        // A virtual-only image and an image transmitted without any placement.
        let mut frame = png_transmit_sequence(43, &tiny_png());
        frame.push_str(&place_image_sequence(43, 2, 1));
        frame.push_str(&png_transmit_sequence(44, &tiny_png()));
        frame.push_str(&image_row(42, 0, 2));
        pane.handle_plugin_bytes(client, frame.into_bytes());
        assert_eq!(kitty_chunks(&mut pane, client).len(), 1);
    }
    for id in &surviving {
        store
            .borrow_mut()
            .zlib_base64_for(*id, None)
            .unwrap()
            .unwrap();
    }
    assert_eq!(store.borrow().image_count(), 6);
    pane.remove_client(ClientId::new(1));
    assert_eq!(store.borrow().image_count(), 3);
    for id in surviving {
        assert!(store.borrow().get(id).is_some());
    }
    drop(pane);
    assert_eq!(store.borrow().image_count(), 0);
    assert_eq!(store.borrow().total_bytes(), 0);
}

#[test]
fn closing_a_plugin_reclaims_quota_for_the_next_plugin() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
    );
    let store = Rc::new(RefCell::new(KittyImageStore::with_quota(1616)));
    pane.kitty_image_store = store.clone();
    pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
    assert_eq!(kitty_chunks(&mut pane, ClientId::new(1)).len(), 1);
    assert_eq!(store.borrow().total_bytes(), 1616);
    pane.handle_plugin_bytes(
        ClientId::new(1),
        png_transmit_sequence(43, &tiny_png()).into_bytes(),
    );
    assert_eq!(
        store.borrow().image_count(),
        1,
        "placed pixels prevent eviction"
    );
    drop(pane);
    assert_eq!(store.borrow().total_bytes(), 0);
    let mut next = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
    );
    next.kitty_image_store = store;
    next.handle_plugin_bytes(ClientId::new(1), image_frame(44, 2, &tiny_png()));
    assert_eq!(kitty_chunks(&mut next, ClientId::new(1)).len(), 1);
}

#[test]
fn plugin_reload_releases_saved_alternate_screen_and_resets_parsers() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 10,
            height: 20,
        }),
    );
    let client = ClientId::new(1);
    for id in 1..4 {
        pane.handle_plugin_bytes(client, image_frame(id, 2, &tiny_png()));
        pane.handle_plugin_bytes(client, b"\x1b[?1049h".to_vec());
        pane.handle_plugin_bytes(client, image_frame(id + 10, 2, &tiny_png()));
        pane.handle_plugin_bytes(client, b"\x1b_Ga=t,m=1;AAAA".to_vec());
        pane.start_loading_indication(LoadingIndication::new("reload".into()));
        assert_eq!(pane.kitty_image_store.borrow().image_count(), 0);
        assert_eq!(pane.kitty_image_store.borrow().total_bytes(), 0);
    }
    pane.start_loading_indication(LoadingIndication::new("reload".into()));
    pane.handle_plugin_bytes(client, image_frame(42, 2, &tiny_png()));
    assert_eq!(kitty_chunks(&mut pane, client).len(), 1);
}

#[test]
fn one_shot_image_survives_delayed_geometry() {
    for (width, height) in [(10, 20), (23, 43)] {
        for delayed in [false, true] {
            let cell = SizeInPixels { width, height };
            let mut pane = test_plugin_pane_with_cell_size(
                10,
                10,
                Offset::default(),
                if delayed { None } else { Some(cell) },
            );
            pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &chunky_png()));
            if delayed {
                assert!(kitty_chunks(&mut pane, ClientId::new(1)).is_empty());
                *pane.character_cell_size.borrow_mut() = Some(cell);
                pane.character_cell_size_possibly_changed();
            }
            assert_eq!(
                kitty_chunks(&mut pane, ClientId::new(1)).len(),
                1,
                "cell={cell:?} delayed={delayed}"
            );
        }
    }
}

#[test]
fn a_plugins_image_survives_the_viewport_reset_between_frames() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 8,
            height: 16,
        }),
    );
    // The pixels are transmitted once; every later frame is only the placeholder cells, which
    // is the whole reason a plugin can afford to redraw an image at ten frames a second.
    pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
    let first = kitty_chunks(&mut pane, ClientId::new(1));
    pane.handle_plugin_bytes(ClientId::new(1), image_row(42, 0, 2).into_bytes());
    let second = kitty_chunks(&mut pane, ClientId::new(1));
    assert_eq!(second.len(), first.len());
    // The placement uid is stable across the rebuild, so the host diff does not retransmit.
    assert_eq!(
        second.first().map(|chunk| chunk.placement_uid),
        first.first().map(|chunk| chunk.placement_uid)
    );
}

#[test]
fn a_plugins_image_is_dropped_when_the_host_terminal_cannot_show_one() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 8,
            height: 16,
        }),
    );
    pane.update_kitty_host_support(KittyHostSupport::ProtocolDisabled);
    pane.handle_plugin_bytes(ClientId::new(1), image_frame(42, 2, &tiny_png()));
    assert!(kitty_chunks(&mut pane, ClientId::new(1)).is_empty());
}

/// A PNG wide enough that its base64 does not fit in one command, so the transmission has to
/// be chunked - which is the shape `yinz-tile`'s `transmit_png` actually emits and the shape
/// agent's wand frames arrive in.
#[test]
fn a_plugin_can_transmit_a_png_across_several_chunks() {
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 8,
            height: 16,
        }),
    );
    pane.handle_plugin_bytes(ClientId::new(1), image_frame(0xb000, 2, &chunky_png()));
    assert_eq!(kitty_chunks(&mut pane, ClientId::new(1)).len(), 1);
}

#[test]
fn a_png_whose_later_chunks_never_arrived_is_not_placed() {
    // The complement of the test above, and what makes it mean something: a PNG cut off after
    // its first chunk does not decode, so a placement appearing at all is proof that the
    // chunks were reassembled rather than that the first one happened to be enough.
    let mut pane = test_plugin_pane_with_cell_size(
        10,
        10,
        Offset::default(),
        Some(SizeInPixels {
            width: 8,
            height: 16,
        }),
    );
    let png = chunky_png();
    let whole = png_transmit_sequence(0xb000, &png);
    // Everything up to the end of the first command, which by itself is not a PNG.
    let first_chunk_end = whole.find("\u{1b}\\").expect("a terminated command") + 2;
    assert!(first_chunk_end < whole.len(), "the fixture did not chunk");
    let mut frame = whole[..first_chunk_end].to_owned();
    frame.push_str(&place_image_sequence(0xb000, 2, 1));
    frame.push_str(&image_row(0xb000, 0, 2));
    pane.handle_plugin_bytes(ClientId::new(1), frame.into_bytes());
    assert!(kitty_chunks(&mut pane, ClientId::new(1)).is_empty());
}

#[test]
fn a_plugins_kitty_replies_are_dropped_rather_than_queued_for_a_pty_it_does_not_have() {
    let mut pane = test_plugin_pane(10, 10, Offset::default());
    // `q` omitted, so the grid would ordinarily answer this - onto a queue nobody drains.
    pane.handle_plugin_bytes(
        ClientId::new(1),
        b"\x1b_Ga=q,i=31,s=1,v=1,t=d,f=24;AAAA\x1b\\".to_vec(),
    );
    let grid = pane.grids.get(&ClientId::new(1)).expect("grid");
    assert!(grid.pending_messages_to_pty.is_empty());
    assert!(grid.pending_forwarded_queries.is_empty());
}
