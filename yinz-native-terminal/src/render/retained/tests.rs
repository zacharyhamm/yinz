use super::*;
use crate::emulation::color::Paints;
use crate::render::scene::test_helpers::Scene;
use crate::render::scene::BlinkPhase;
use crate::render::scene::SceneInputs;
use crate::testing::painter::Painter;
use crate::text::font::CellChange;
use crate::text::font::{FontStack, DEFAULT_FONT_SIZE};
use yinz_types::structured_render::{
    FrameBuilder, GraphicsMedium, GraphicsRecord, GRAPHICS_FORMAT_RGBA8,
};

fn cache() -> GlyphCache {
    GlyphCache::new(FontStack::embedded(DEFAULT_FONT_SIZE).expect("no embedded font"))
}

fn refreshed(scene: &mut RetainedScene, state: &TerminalState, cache: &mut GlyphCache) {
    scene.refresh(
        SceneInputs {
            state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        cache,
    );
}

fn rebuilt(state: &TerminalState, cache: &mut GlyphCache) -> Scene {
    scene::test_helpers::build_at(
        state,
        cache,
        BlinkPhase::On,
        &Paints::default(),
        CursorOptions::default(),
        None,
        None,
    )
}

fn painted() -> TerminalState {
    Painter::state(4, 8, |painter| painter.text(0, 0, "hello"))
}

#[test]
fn a_retained_scene_starts_by_building_every_row() {
    let mut cache = cache();
    let state = painted();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.rebuilt_everything());
    assert_eq!(retained.flatten(), rebuilt(&state, &mut cache));
}

#[test]
fn an_unmarked_refresh_rebuilds_nothing() {
    let mut cache = cache();
    let state = painted();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.rebuilt_everything());
    assert!(retained.rebuilt_rows().is_empty());
    assert_eq!(retained.flatten(), rebuilt(&state, &mut cache));
}

#[test]
fn only_the_rows_a_frame_touched_are_rebuilt() {
    let mut cache = cache();
    let mut state = painted();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);

    Painter::apply_marking(&mut state, &mut retained, |painter| {
        painter.text(2, 0, "second")
    });
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.rebuilt_everything());
    assert_eq!(retained.rebuilt_rows(), [2]);
    assert_eq!(retained.flatten(), rebuilt(&state, &mut cache));
}

#[test]
fn a_new_glyph_cache_rebuilds_every_row() {
    let mut cache = cache();
    let state = painted();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.rebuilt_everything());

    let mut rebuilt_cache = self::cache();
    refreshed(&mut retained, &state, &mut rebuilt_cache);
    assert!(retained.rebuilt_everything());
}

#[test]
fn a_selection_paints_its_cells_in_reversed_colours_and_rebuilds_only_its_rows() {
    let mut cache = cache();
    let state = Painter::state(4, 8, |painter| {
        painter.text(1, 0, "hello");
        painter.text(2, 0, "world");
    });
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    let width = cache.metrics().width as i32;
    let height = cache.metrics().height as i32;
    let foreground = Paints::default().foreground;
    let filled = |scene: &Scene, row: usize, col: usize| {
        let y = row as i32 * height;
        scene
            .rects
            .iter()
            .any(|rect| rect.x == col as i32 * width && rect.y == y && rect.color == foreground)
    };
    assert!(!filled(&retained.flatten(), 1, 2));

    retained.set_selection(Some(Selection::at(1, 2).extended_to(2, 1)));
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(retained.rebuilt_rows(), [1, 2]);
    let selected = retained.flatten();
    assert!(filled(&selected, 1, 2));
    assert!(filled(&selected, 1, 7));
    assert!(filled(&selected, 2, 0));
    assert!(filled(&selected, 2, 1));
    assert!(!filled(&selected, 1, 1));
    assert!(!filled(&selected, 2, 2));

    retained.set_selection(None);
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(retained.rebuilt_rows(), [1, 2]);
    assert_eq!(retained.flatten(), rebuilt(&state, &mut cache));
}

#[test]
fn a_row_index_past_the_viewport_marks_everything() {
    let mut cache = cache();
    let state = painted();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    retained.mark(&Damage::of([9]));
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.rebuilt_everything());
}

#[test]
fn a_cursor_that_moves_marks_the_row_it_left_and_the_row_it_entered() {
    let mut cache = cache();
    let mut state = Painter::state(4, 8, |painter| {
        painter.text(0, 0, "hello");
        painter.cursor(0, 0, crate::emulation::screen_buffer::CursorShape::Block);
    });
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);

    Painter::apply_marking(&mut state, &mut retained, |painter| {
        painter.cursor(3, 4, crate::emulation::screen_buffer::CursorShape::Block)
    });
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(retained.rebuilt_rows(), [0, 3]);
    assert_eq!(retained.flatten(), rebuilt(&state, &mut cache));
}

#[test]
fn a_blink_flip_damages_only_the_rows_that_blink() {
    let mut cache = cache();
    let state = Painter::state(4, 8, |painter| {
        painter.text(0, 0, "steady");
        painter.styled(2, 0, "blinks", |cell| {
            cell.attrs |= yinz_types::structured_render::ATTR_SLOW_BLINK
        });
    });
    let damage = blink_damage(&state, CursorOptions::default());
    assert_eq!(damage, Damage::Rows(vec![2]));

    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    retained.mark(&damage);
    retained.refresh(
        SceneInputs {
            state: &state,
            phase: BlinkPhase::Off,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        &mut cache,
    );
    assert_eq!(retained.rebuilt_rows(), [2]);
    assert_eq!(
        retained.flatten(),
        scene::test_helpers::build_at(
            &state,
            &mut cache,
            BlinkPhase::Off,
            &Paints::default(),
            CursorOptions::default(),
            None,
            None,
        )
    );
}

fn tiny_cache() -> GlyphCache {
    GlyphCache::tiny(FontStack::embedded(DEFAULT_FONT_SIZE).expect("no embedded font"))
}

fn crowded() -> crate::emulation::terminal::TerminalState {
    Painter::state(4, 40, |painter| {
        painter.text(0, 0, "abcdefghijklmnopqrstuvwxyz0123456789");
        painter.text(1, 0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ!?#$%&*+-=");
    })
}

#[test]
fn a_refresh_that_fills_the_atlas_flushes_it_and_rebuilds_against_the_fresh_one() {
    let mut cache = tiny_cache();
    let state = crowded();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(cache.flushes(), 1, "a full atlas must be renewed once");
    assert!(retained.rebuilt_everything());
    assert_eq!(retained.flatten(), rebuilt(&state, &mut tiny_cache()));
}

fn overflowing() -> crate::emulation::terminal::TerminalState {
    Painter::state(6, 40, |painter| {
        painter.text(0, 0, "abcdefghijklmnopqrstuvwxyz0123456789");
        painter.text(1, 0, "ABCDEFGHIJKLMNOPQRSTUVWXYZ!?#$%&*+-=");
        painter.text(
            2,
            0,
            "\u{4f60}\u{597d}\u{4e16}\u{754c}\u{6f22}\u{5b57}\u{4e2d}\u{6587}\u{65e5}\u{672c}",
        );
        painter.text(
            3,
            0,
            "\u{3042}\u{3044}\u{3046}\u{3048}\u{304a}\u{30a2}\u{30a4}\u{30a6}\u{30a8}\u{30aa}",
        );
    })
}

#[test]
fn a_screen_the_atlas_cannot_hold_is_not_flushed_on_every_refresh() {
    let mut cache = tiny_cache();
    let state = overflowing();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    assert!(
        cache.exhausted(),
        "the screen must overflow even a freshly flushed tiny atlas for this test to mean anything"
    );
    let flushes = cache.flushes();
    for _ in 0..20 {
        retained.mark(&Damage::Everything);
        refreshed(&mut retained, &state, &mut cache);
    }
    assert_eq!(
        cache.flushes(),
        flushes,
        "an overflowing screen flushed the atlas again straight away"
    );
}

#[test]
fn a_screen_the_atlas_holds_easily_is_never_flushed() {
    let mut cache = tiny_cache();
    let state = Painter::state(4, 8, |painter| painter.text(0, 0, "aaa"));
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(cache.flushes(), 0);
}

fn graphics(state: &mut crate::emulation::terminal::TerminalState, records: &[GraphicsRecord]) {
    let size = state.size();
    let mut builder = FrameBuilder::new(size.cols as u16, size.rows as u16, 0);
    builder.extend_graphics(records.iter());
    state
        .apply_frame(&builder.finish())
        .expect("a usable frame");
}

fn placement(cell_x: u32) -> GraphicsRecord {
    GraphicsRecord::Placement {
        image_id: 1,
        placement_id: 1,
        cell_x,
        cell_y: 1,
        offset_x: 0,
        offset_y: 0,
        source_x: 0,
        source_y: 0,
        source_width: 2,
        source_height: 2,
        z: 0,
    }
}

fn with_an_image(cell_x: u32) -> crate::emulation::terminal::TerminalState {
    let mut state = crate::emulation::terminal::TerminalState::new(4, 8);
    state.set_cell_size(8, 20);
    graphics(
        &mut state,
        &[
            GraphicsRecord::Residency {
                id: 1,
                width: 2,
                height: 2,
                format: GRAPHICS_FORMAT_RGBA8,
                medium: GraphicsMedium::Inline,
                byte_len: 16,
                data: [10, 20, 30, 255].repeat(4),
            },
            placement(cell_x),
        ],
    );
    state
}

fn see_through() -> Transparency {
    Transparency {
        opacity: 0.8,
        mode: yinz_types::input::native_terminal::OpacityMode::Background,
    }
}

fn rects_in_row(scene: &RetainedScene, row: usize) -> usize {
    scene.rows()[row].rects.len()
}

#[test]
fn a_see_through_background_draws_only_the_backgrounds_a_program_chose() {
    use yinz_types::structured_render::{WireColor, ATTR_REVERSE};
    let mut cache = cache();
    let state = Painter::state(4, 8, |painter| {
        painter.text(0, 0, "plain");
        painter.styled(1, 0, " ", |cell| cell.bg = WireColor::Rgb(0, 0, 0).pack());
        painter.styled(2, 0, " ", |cell| cell.attrs |= ATTR_REVERSE);
        painter.styled(3, 0, " ", |cell| cell.bg = WireColor::Named(1).pack());
    });

    let mut opaque = RetainedScene::new();
    refreshed(&mut opaque, &state, &mut cache);
    assert_eq!(rects_in_row(&opaque, 0), 0);
    assert_eq!(
        rects_in_row(&opaque, 1),
        0,
        "a solid window folds an explicit copy of the default into the clear"
    );

    let mut clear = RetainedScene::new();
    clear.set_transparency(see_through());
    refreshed(&mut clear, &state, &mut cache);
    assert_eq!(
        rects_in_row(&clear, 0),
        0,
        "a default cell must let the clear through"
    );
    assert_eq!(
        rects_in_row(&clear, 1),
        1,
        "an explicit background equal to the default"
    );
    assert_eq!(rects_in_row(&clear, 2), 1, "a reverse-video cell");
    assert_eq!(rects_in_row(&clear, 3), 1, "a colored cell");
}

#[test]
fn hidden_text_on_a_see_through_background_leaves_no_ink() {
    use yinz_types::structured_render::{
        WireColor, ATTR_HIDDEN, UNDERLINE_SHIFT, UNDERLINE_STRAIGHT,
    };
    let mut cache = cache();
    let state = Painter::state(2, 8, |painter| {
        painter.styled(0, 0, "MM", |cell| {
            cell.attrs |= ATTR_HIDDEN | (UNDERLINE_STRAIGHT << UNDERLINE_SHIFT)
        });
        painter.styled(1, 0, "MM", |cell| {
            cell.attrs |= ATTR_HIDDEN;
            cell.bg = WireColor::Named(1).pack();
        });
    });
    let mut opaque = RetainedScene::new();
    refreshed(&mut opaque, &state, &mut cache);
    assert!(!opaque.rows()[0].glyphs.is_empty() || !opaque.rows()[0].rects.is_empty());

    let mut clear = RetainedScene::new();
    clear.set_transparency(see_through());
    refreshed(&mut clear, &state, &mut cache);
    assert!(clear.rows()[0].glyphs.is_empty(), "hidden glyphs showed");
    assert!(
        clear.rows()[0].rects.is_empty(),
        "a hidden underline showed"
    );
    assert_eq!(clear.rows()[1].rects.len(), 2);
}

#[test]
fn a_change_of_transparency_rebuilds_every_row() {
    let mut cache = cache();
    let state = painted();
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.rebuilt_everything());

    retained.set_transparency(see_through());
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.rebuilt_everything());
    assert_eq!(retained.transparency(), see_through());

    retained.set_transparency(see_through());
    refreshed(&mut retained, &state, &mut cache);
    assert!(
        !retained.rebuilt_everything(),
        "an unchanged setting rebuilt"
    );

    retained.set_transparency(Transparency {
        mode: yinz_types::input::native_terminal::OpacityMode::Everything,
        ..see_through()
    });
    refreshed(&mut retained, &state, &mut cache);
    assert!(
        retained.rebuilt_everything(),
        "a mode change did not rebuild"
    );
}

#[test]
fn a_graphics_state_that_has_not_moved_keeps_the_image_list_it_emitted() {
    let mut cache = cache();
    let state = with_an_image(1);
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.replaced_images());
    assert_eq!(retained.images().len(), 1);

    refreshed(&mut retained, &state, &mut cache);
    assert!(
        !retained.replaced_images(),
        "nothing about the graphics changed, so the placements must not be re-emitted"
    );
    assert_eq!(retained.images().len(), 1);
}

#[test]
fn a_placement_that_moves_is_re_emitted() {
    let mut cache = cache();
    let mut state = with_an_image(1);
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    let before = retained.images()[0].x;

    graphics(&mut state, &[placement(4)]);
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.replaced_images());
    assert_ne!(retained.images()[0].x, before);
}

#[test]
fn a_cell_size_that_changes_re_emits_the_placements() {
    let mut cache = cache();
    let state = with_an_image(1);
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.replaced_images());

    let mut zoomed =
        GlyphCache::new(FontStack::embedded(DEFAULT_FONT_SIZE * 2.0).expect("no embedded font"));
    refreshed(&mut retained, &state, &mut zoomed);
    assert!(
        retained.replaced_images(),
        "an image is placed in pixels, so a new cell size moves it"
    );
}

#[test]
fn captured_context_damage_erases_highlight_on_an_undamaged_selected_row() {
    use crate::emulation::selection::LocalSelection;
    let mut state = Painter::state(3, 10, |p| {
        p.text(0, 0, "0123456789");
        p.text(1, 0, "ABCDsuffix");
    });
    let mut selection = LocalSelection::default();
    selection.start(1, 0);
    selection.extend(1, 3);
    selection.resizing(&state, 4, 10);
    let repaint = Painter::frame(4, 10, |p| {
        p.full_repaint();
        p.text(0, 0, "0123456789");
        p.text(1, 0, "ABCDsuffix");
    });
    selection.apply_frame(&mut state, &repaint).unwrap();
    assert_eq!(
        selection.range(),
        Some(Selection::at(1, 0).extended_to(1, 3))
    );
    let mut cache = cache();
    let generation = cache.generation();
    let mut retained = RetainedScene::new();
    retained.set_selection(selection.range());
    refreshed(&mut retained, &state, &mut cache);
    let selected_scene = retained.flatten();
    // Only context on row 0 changes. App's draw order must also dirty row 1
    // when set_selection removes its previously verified highlight.
    let frame = Painter::frame(4, 10, |p| p.text(0, 2, "!"));
    let applied = selection.apply_frame(&mut state, &frame).unwrap();
    assert_eq!(applied.damage, Damage::Rows(vec![0]));
    retained.mark(&applied.damage);
    assert_eq!(selection.range(), None);
    assert_eq!(selection.text(state.screen()).as_deref(), Some("ABCD"));
    retained.set_selection(selection.range());
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(retained.rebuilt_rows(), [0, 1]);
    assert_ne!(retained.flatten(), selected_scene);
    assert_eq!(retained.flatten(), rebuilt(&state, &mut cache));
    assert_eq!(cache.generation(), generation);
}

#[test]
fn cell_metrics_generation_refreshes_retained_rows_and_selection_rectangles() {
    use crate::emulation::selection::Selection;
    use crate::text::font::{CellAdjust, FontOptions};
    let options = FontOptions {
        system_fonts: false,
        ..Default::default()
    };
    let mut cache = GlyphCache::new(options.stack(1.0).unwrap());
    let state = Painter::state(2, 12, |p| {
        p.wide(1, 1, '中');
        p.wide(1, 3, '😀');
        p.cluster(1, 5, "👩‍💻", 2);
        p.cluster(1, 7, "e\u{301}", 1);
    });
    let selection = Selection::at(1, 1).extended_to(1, 7);
    let copied = selection.text(state.screen());
    let mut retained = RetainedScene::new();
    retained.set_selection(Some(selection));
    refreshed(&mut retained, &state, &mut cache);
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.rebuilt_rows().is_empty());
    for (width, height, w, h) in [
        (1.5, 1.25, 12, 25),
        (0.5, 0.5, 4, 10),
        (3.0, 3.0, 24, 60),
        (1.0, 1.0, 8, 20),
    ] {
        let options = FontOptions {
            cell: CellAdjust {
                cell_width: width,
                line_height: height,
                ..Default::default()
            },
            ..options.clone()
        };
        assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        // No explicit damage: the renewed cache generation must invalidate retained rows.
        refreshed(&mut retained, &state, &mut cache);
        assert!(retained.rebuilt_everything());
        let live = retained.flatten();
        assert_eq!((live.width, live.height), (12 * w, 2 * h));
        for col in 1..8 {
            assert!(live.rects.iter().any(|r| r.x == (col * w) as i32
                && r.y == h as i32
                && r.width == w
                && r.height == h
                && r.color == Paints::default().foreground));
        }
        let mut fresh_cache = GlyphCache::new(options.stack(1.0).unwrap());
        let mut fresh = RetainedScene::new();
        fresh.set_selection(Some(selection));
        refreshed(&mut fresh, &state, &mut fresh_cache);
        assert_eq!(live, fresh.flatten());
        assert_eq!(selection.text(state.screen()), copied);
    }
}

#[test]
fn cell_metrics_retained_clusters_refresh_sprites_and_keep_copy() {
    use crate::emulation::selection::Selection;
    use crate::render::atlas::{ClusterLookup, GlyphKey};
    use crate::text::font::{CellAdjust, FaceStyle, FontOptions};
    let options = FontOptions {
        system_fonts: false,
        ..Default::default()
    };
    let state = Painter::state(3, 12, |p| {
        p.cluster(1, 1, "e\u{301}", 1);
        p.cluster(1, 3, "👩‍💻", 2);
        p.cluster(1, 6, "👍🏽", 2);
    });
    let selection = Selection::at(1, 1).extended_to(1, 7);
    let copied = selection.text(state.screen());
    let mut cache = GlyphCache::new(options.stack(1.0).unwrap());
    let mut retained = RetainedScene::new();
    retained.set_selection(Some(selection));
    refreshed(&mut retained, &state, &mut cache);
    for (width, height) in [
        (0.75, 1.0),
        (0.5, 1.0),
        (0.5, 1.25),
        (1.5, 1.25),
        (1.0, 1.0),
        (1.001, 1.001),
    ] {
        refreshed(&mut retained, &state, &mut cache);
        assert!(retained.rebuilt_rows().is_empty());
        let requested = FontOptions {
            cell: CellAdjust {
                cell_width: width,
                line_height: height,
                ..Default::default()
            },
            ..options.clone()
        };
        let changed = cache.update_cell(&requested);
        refreshed(&mut retained, &state, &mut cache);
        assert_eq!(
            retained.rebuilt_everything(),
            changed == CellChange::GlyphPlacement
        );
        let live = retained.flatten();
        for (text, budget) in [("e\u{301}", 1), ("👩‍💻", 2), ("👍🏽", 2)] {
            let ClusterLookup::Sprite(sprite) =
                cache.cluster_glyph(FaceStyle::Regular, &GlyphKey::of(text), budget)
            else {
                panic!("stale cluster fallback: {text} at {width}");
            };
            let quads = if sprite.content == crate::text::font::GlyphContent::Mask {
                &live.glyphs
            } else {
                &live.color_glyphs
            };
            // Cell clipping can narrow a sprite's atlas subrectangle; it must
            // still refer to the complete cluster's newly generated pixels.
            assert!(quads.iter().any(|q| q.entry.y == sprite.entry.y
                && q.entry.height == sprite.entry.height
                && q.entry.x >= sprite.entry.x
                && q.entry.x + q.entry.width <= sprite.entry.x + sprite.entry.width));
        }
        let mut fresh_cache = GlyphCache::new(requested.stack(1.0).unwrap());
        let mut fresh = RetainedScene::new();
        fresh.set_selection(Some(selection));
        refreshed(&mut fresh, &state, &mut fresh_cache);
        assert_eq!(live, fresh.flatten());
        assert_eq!(selection.text(state.screen()), copied);
    }
}

#[test]
fn baseline_offset_retains_kitty_sixel_and_held_image_placements() {
    use crate::render::scene::CellRect;
    use crate::testing::painter::{residency, sixel_chunk, Place};
    use crate::text::font::FontOptions;
    let options = FontOptions {
        system_fonts: false,
        ..Default::default()
    };
    let mut cache = GlyphCache::new(options.stack(1.0).unwrap());
    let state = Painter::state(4, 16, |p| {
        p.text(0, 0, "Ag -> fi");
        p.graphics(&[
            residency(1, 8, 8, [0, 255, 0, 255]),
            Place::of(1, 8, 8).at(1, 2).cropped(2, 1, 6, 7).record(),
            residency(2, 8, 8, [0, 0, 255, 255]),
            Place::of(2, 8, 8).at(2, 10).record(),
            sixel_chunk(3, 4, 8, 6, "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\"),
        ]);
    });
    let mut retained = RetainedScene::new();
    retained.set_held_out(vec![HeldOut {
        content: CellRect {
            x: 10,
            y: 2,
            cols: 2,
            rows: 1,
        },
        covers: vec![],
    }]);
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(retained.all_images().len(), 3);
    assert_eq!(retained.images().len(), 2);
    let images = retained.all_images().to_vec();
    let visible = retained.images().to_vec();
    let resident = retained.resident_images().to_vec();
    let dimensions = (cache.metrics().width, cache.metrics().height);
    for offset in [2.0, -2.0, 0.0] {
        let before = cache.generation();
        let mut requested = options.clone();
        requested.cell.baseline_offset = offset;
        assert_eq!(cache.update_cell(&requested), CellChange::GlyphPlacement);
        refreshed(&mut retained, &state, &mut cache);
        assert_ne!(cache.generation(), before);
        assert!(retained.rebuilt_everything());
        assert!(
            !retained.replaced_images(),
            "baseline {offset} re-emitted image placements"
        );
        assert_eq!(retained.all_images(), images);
        assert_eq!(retained.images(), visible);
        assert_eq!(retained.resident_images(), resident);
        assert_eq!((cache.metrics().width, cache.metrics().height), dimensions);
        let mut fresh = RetainedScene::new();
        fresh.set_held_out(retained.held.clone());
        let mut fresh_cache = GlyphCache::new(requested.stack(1.0).unwrap());
        refreshed(&mut fresh, &state, &mut fresh_cache);
        assert_eq!(retained.flatten(), fresh.flatten());
    }
}

#[test]
fn baseline_offset_image_placement_inputs_still_refresh() {
    use crate::render::scene::CellRect;
    use crate::testing::painter::{residency, sixel_chunk, Place};
    use crate::text::font::FontOptions;
    use yinz_types::structured_render::{GeometryRecord, PaneRect, PANE_SELECTABLE};
    let mut options = FontOptions {
        system_fonts: false,
        ..Default::default()
    };
    let mut cache = GlyphCache::new(options.stack(1.0).unwrap());
    let mut state = Painter::state(4, 16, |p| {
        p.graphics(&[
            residency(1, 32, 8, [0, 255, 0, 255]),
            Place::of(1, 32, 8).at(1, 2).record(),
            sixel_chunk(3, 4, 8, 6, "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\"),
        ])
    });
    let mut retained = RetainedScene::new();
    refreshed(&mut retained, &state, &mut cache);
    for change in [
        "width", "height", "move", "revision", "pane", "held", "unheld", "delete",
    ] {
        let old = retained.images().to_vec();
        match change {
            "width" => {
                options.cell.cell_width = 1.5;
                assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
            },
            "height" => {
                options.cell.line_height = 1.5;
                assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
            },
            "move" => Painter::apply(&mut state, |p| {
                p.graphics(&[Place::of(1, 32, 8).at(1, 3).record()])
            }),
            "revision" => Painter::apply(&mut state, |p| {
                p.graphics(&[residency(1, 32, 8, [0, 0, 255, 255])])
            }),
            "pane" => Painter::apply(&mut state, |p| {
                p.geometry(&GeometryRecord {
                    panes: vec![PaneRect {
                        x: 0,
                        y: 0,
                        cols: 4,
                        rows: 4,
                        top: 0,
                        bottom: 0,
                        left: 0,
                        right: 0,
                        flags: PANE_SELECTABLE,
                    }],
                })
            }),
            "held" => retained.set_held_out(vec![HeldOut {
                content: CellRect {
                    x: 3,
                    y: 1,
                    cols: 2,
                    rows: 1,
                },
                covers: vec![],
            }]),
            "unheld" => retained.set_held_out(vec![]),
            "delete" => Painter::apply(&mut state, |p| p.full_repaint()),
            _ => unreachable!(),
        }
        refreshed(&mut retained, &state, &mut cache);
        assert!(retained.replaced_images(), "{change}");
        assert_ne!(retained.images(), old, "{change}");
        let mut fresh = RetainedScene::new();
        fresh.set_held_out(retained.held.clone());
        let mut fresh_cache = GlyphCache::new(options.stack(1.0).unwrap());
        refreshed(&mut fresh, &state, &mut fresh_cache);
        assert_eq!(retained.all_images(), fresh.all_images(), "{change}");
        assert_eq!(retained.images(), fresh.images(), "{change}");
        assert_eq!(
            retained.resident_images(),
            fresh.resident_images(),
            "{change}"
        );
        if change == "pane" {
            assert_eq!(
                retained
                    .images()
                    .iter()
                    .find(|q| q.key == ImageKey::Kitty(1))
                    .unwrap()
                    .width,
                cache.metrics().width
            );
        }
        if change == "delete" {
            assert!(retained.images().is_empty());
            assert!(retained.resident_images().is_empty());
        }
    }
}

fn baseline_offset_atlas_retry_pressure(cache: &mut GlyphCache, ordinary: bool) {
    use crate::text::font::FaceStyle;
    // Distinct raster budgets exercise the real insertion path. The ordinary
    // atlas reaches the production entry ceiling; the tiny one runs out of area.
    for budget in 1..=32768 {
        cache.glyph(FaceStyle::Regular, 'A', budget);
        if cache.exhausted() {
            break;
        }
    }
    assert!(cache.exhausted());
    assert!(cache.may_flush(Instant::now()));
    if ordinary {
        assert_eq!(cache.entries(), 32768);
    } else {
        assert!(cache.entries() < 32768);
    }
}

fn baseline_offset_atlas_retry_state() -> TerminalState {
    use crate::testing::painter::{residency, sixel_chunk, Place};
    Painter::state(4, 8, |p| {
        p.text(0, 0, "A");
        p.graphics(&[
            residency(1, 24, 8, [255; 4]),
            Place::of(1, 24, 8).at(1, 1).record(),
            sixel_chunk(3, 4, 8, 6, "\x1bP0;1;0q\"1;1;8;6#0;2;100;0;0!8~\x1b\\"),
        ]);
    })
}

fn baseline_offset_atlas_retry_held() -> Vec<HeldOut> {
    vec![HeldOut {
        content: scene::CellRect {
            x: 1,
            y: 1,
            cols: 3,
            rows: 1,
        },
        covers: vec![],
    }]
}

fn baseline_offset_atlas_retry_change(change: &str, ordinary: bool, retry: bool) {
    use crate::testing::painter::{residency, sixel_chunk, Place};
    use crate::text::font::FontOptions;
    use yinz_types::structured_render::{GeometryRecord, PaneRect, PANE_SELECTABLE};
    let mut options = FontOptions {
        system_fonts: false,
        ..Default::default()
    };
    let make_cache = |options: &FontOptions| {
        let fonts = options.stack(1.0).unwrap();
        if ordinary {
            GlyphCache::new(fonts)
        } else {
            GlyphCache::tiny(fonts)
        }
    };
    let mut cache = make_cache(&options);
    let mut state = baseline_offset_atlas_retry_state();
    let mut retained = RetainedScene::new();
    let selection = Selection::at(0, 0).extended_to(0, 1);
    retained.set_selection(Some(selection));
    if change == "unheld" {
        retained.set_held_out(baseline_offset_atlas_retry_held());
    }
    refreshed(&mut retained, &state, &mut cache);
    assert!(retained.replaced_images(), "first image refresh");
    assert!(!cache.exhausted(), "visible scene must fit the tiny atlas");
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.replaced_images());
    let identity = retained.identity();
    let placed = retained.placed;
    let old = retained.flatten();
    let copied = selection.text(state.screen());
    let cells: Vec<_> = (0..4)
        .flat_map(|r| (0..8).map(move |c| (r, c)))
        .map(|(r, c)| state.screen().cell(r, c))
        .collect();
    match change {
        "move" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[Place::of(1, 24, 8).at(1, 4).record()]);
        }),
        "revision" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[residency(1, 24, 8, [0, 0, 255, 255])]);
        }),
        "delete" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.full_repaint();
            p.text(0, 0, "A");
        }),
        "add" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[
                residency(2, 4, 4, [0, 255, 0, 255]),
                Place::of(2, 4, 4).at(2, 6).record(),
            ]);
        }),
        "sixel" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.graphics(&[sixel_chunk(
                3,
                4,
                8,
                6,
                "\x1bP0;1;0q\"1;1;8;6#0;2;0;0;100!8~\x1b\\",
            )]);
        }),
        "pane" => Painter::apply_marking(&mut state, &mut retained, |p| {
            p.geometry(&GeometryRecord {
                panes: vec![PaneRect {
                    x: 0,
                    y: 0,
                    cols: 2,
                    rows: 4,
                    top: 0,
                    bottom: 0,
                    left: 0,
                    right: 0,
                    flags: PANE_SELECTABLE,
                }],
            });
        }),
        "width" => {
            options.cell.cell_width = 1.5;
            assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        },
        "height" => {
            options.cell.line_height = 1.5;
            assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        },
        "held" => retained.set_held_out(baseline_offset_atlas_retry_held()),
        "unheld" => retained.set_held_out(vec![]),
        "baseline" => {
            options.cell.baseline_offset = 2.0;
            assert_eq!(cache.update_cell(&options), CellChange::GlyphPlacement);
        },
        "unchanged" => {},
        _ => unreachable!(),
    }
    // Cell changes themselves flush; only pressure after those changes can
    // establish that this public refresh, rather than update_cell, renews it.
    let flushes = cache.flushes();
    let generation = cache.generation();
    retained.mark_row(2);
    if retry {
        baseline_offset_atlas_retry_pressure(&mut cache, ordinary);
    }
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(cache.flushes(), flushes + u64::from(retry), "{change}");
    assert_eq!(cache.generation() != generation, retry, "{change}");
    assert_eq!(retained.identity(), identity);
    assert!(!cache.exhausted(), "post-renewal scene must fit: {change}");
    let changed = !matches!(change, "unchanged" | "baseline");
    assert_eq!(
        retained.replaced_images(),
        changed,
        "{change}: image damage lost during retry"
    );
    if matches!(change, "held" | "unheld" | "unchanged" | "baseline") {
        assert_eq!(
            retained.placed, placed,
            "held-only/text-only changes keep placement identity"
        );
    }
    if retry {
        assert!(retained.rebuilt_everything());
    } else {
        assert!(retained.rebuilt_rows().contains(&2));
    }
    assert_eq!(selection.text(state.screen()), copied);
    for (i, cell) in cells.iter().enumerate() {
        assert_eq!(state.screen().cell(i / 8, i % 8), *cell);
    }
    let mut fresh_cache = make_cache(&options);
    let mut fresh = RetainedScene::new();
    fresh.set_selection(Some(selection));
    fresh.set_held_out(retained.held.clone());
    refreshed(&mut fresh, &state, &mut fresh_cache);
    assert!(!fresh_cache.exhausted());
    assert_eq!(retained.flatten(), fresh.flatten(), "{change}");
    assert_eq!(retained.all_images(), fresh.all_images());
    if change == "delete" {
        assert!(retained.images().is_empty());
        assert!(retained.resident_images().is_empty());
    } else if change == "pane" {
        assert_eq!(
            retained
                .images()
                .iter()
                .find(|q| q.key == ImageKey::Kitty(1))
                .unwrap()
                .width,
            cache.metrics().width
        );
    } else if !changed {
        assert_eq!(retained.images(), old.images);
        assert_eq!(retained.resident_images(), old.resident_images);
    }
    let current = retained.flatten();
    refreshed(&mut retained, &state, &mut cache);
    assert!(
        !retained.replaced_images(),
        "damage must not persist across public calls"
    );
    assert!(!retained.rebuilt_everything());
    assert!(retained.rebuilt_rows().is_empty());
    assert_eq!(retained.flatten(), current);
    assert_eq!(cache.flushes(), flushes + u64::from(retry));
}

#[test]
fn baseline_offset_atlas_retry_changed_images_keep_damage() {
    for change in [
        "move", "revision", "delete", "add", "sixel", "pane", "width", "height", "held", "unheld",
    ] {
        baseline_offset_atlas_retry_change(change, false, true);
    }
}

#[test]
fn baseline_offset_atlas_retry_ordinary_entry_limit_keeps_image_damage() {
    baseline_offset_atlas_retry_change("move", true, true);
}

#[test]
fn baseline_offset_atlas_retry_unchanged_images_remain_clean() {
    for change in ["unchanged", "baseline"] {
        baseline_offset_atlas_retry_change(change, false, true);
    }
}

#[test]
fn baseline_offset_atlas_retry_single_pass_keeps_normal_image_damage() {
    baseline_offset_atlas_retry_change("move", false, false);
}

#[test]
fn baseline_offset_atlas_retry_first_refresh_keeps_image_damage() {
    let mut cache = tiny_cache();
    let state = baseline_offset_atlas_retry_state();
    let mut retained = RetainedScene::new();
    baseline_offset_atlas_retry_pressure(&mut cache, false);
    refreshed(&mut retained, &state, &mut cache);
    assert_eq!(cache.flushes(), 1);
    assert!(!cache.exhausted());
    assert_eq!(retained.all_images().len(), 2);
    assert!(retained.replaced_images());
    refreshed(&mut retained, &state, &mut cache);
    assert!(!retained.replaced_images());
}
