use super::sample::*;
use super::*;
use crate::emulation::color::Paints;
use crate::render::retained::RetainedScene;
use crate::render::scene::BlinkPhase;
use crate::text::font::{FontStack, DEFAULT_FONT_SIZE};

#[test]
fn the_offset_starts_where_the_content_was_and_eases_to_nothing_over_the_duration() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    assert_eq!(scene.scroll.offset_of(0, start), Some(-60.0));
    let halfway = scene.scroll.offset_of(0, start + DURATION / 2).unwrap();
    assert!(halfway > -60.0 && halfway < 0.0, "{}", halfway);
    assert_eq!(scene.scroll.offset_of(0, start + DURATION), Some(0.0));
    assert!(!scene.scroll.animating(1));

    assert!(!scene.scroll.retire(start + DURATION / 2));
    assert!(scene.scroll.retire(start + DURATION));
    assert!(!scene.scroll.is_active());
}

#[test]
fn scrolling_the_other_way_starts_below_and_slides_up() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(-2, start);
    assert_eq!(scene.scroll.offset_of(0, start), Some(40.0));
}

#[test]
fn the_strip_holds_the_rows_that_just_left_nearest_first() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let animation = &scene.scroll.panes[0];
    let below: Vec<String> = animation
        .below
        .iter()
        .map(|cells| Scene::text(&cells[..4]))
        .collect();
    assert_eq!(below, vec!["L107", "L108", "L109"]);
    assert!(animation.above.is_empty());

    scene.scroll(-5, start + DURATION / 4);
    let animation = &scene.scroll.panes[0];
    let above: Vec<String> = animation
        .above
        .iter()
        .map(|cells| Scene::text(&cells[..3]))
        .collect();
    assert_eq!(above, vec!["L10", "L10", "L09", "L09", "L09"]);
    assert_eq!(Scene::text(&animation.above[3][..4]), "L098");
    assert_eq!(Scene::text(&animation.above[4][..4]), "L097");
    assert!(animation.below.is_empty());
}

#[test]
fn the_strip_covers_every_row_that_enters_in_the_usual_cases() {
    let mut scene = Scene::new();
    let start = Instant::now();
    let check = |scene: &Scene, at: Instant| {
        assert_eq!(
            scene.scroll.uncovered_rows(0, at, CELL_HEIGHT),
            Some(0),
            "rows missing at {:?}",
            at - start
        );
    };
    scene.scroll(3, start);
    for step in 0..10 {
        check(&scene, start + DURATION * step / 10);
    }
    let later = start + DURATION / 3;
    scene.scroll(3, later);
    for step in 0..10 {
        check(&scene, later + DURATION * step / 10);
    }
    let reversed = later + DURATION / 2;
    scene.scroll(-4, reversed);
    for step in 0..10 {
        check(&scene, reversed + DURATION * step / 10);
    }
    let again = reversed + DURATION / 5;
    scene.scroll(-1, again);
    for step in 0..10 {
        check(&scene, again + DURATION * step / 10);
    }
}

#[test]
fn a_hint_during_an_animation_adds_to_the_remaining_offset() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let later = start + DURATION / 2;
    let remaining = scene.scroll.offset_of(0, later).unwrap();
    scene.scroll(2, later);
    assert_eq!(scene.scroll.offset_of(0, later), Some(remaining - 40.0));
    assert_eq!(scene.scroll.offset_of(0, later + DURATION), Some(0.0));
}

#[test]
fn an_offset_beyond_the_pane_height_drops_the_animation() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(6, start);
    assert!(scene.scroll.is_active());
    scene.scroll(6, start);
    assert!(!scene.scroll.is_active());

    scene.scroll(11, start);
    assert!(!scene.scroll.is_active());
}

#[test]
fn a_frame_that_changes_the_geometry_starts_no_animation() {
    let mut scene = Scene::new();
    let start = Instant::now();
    let mut moved = panes();
    moved.panes[2].x = 5;
    scene.base -= 3;
    let bytes = frame(scene.seq, false, Some(moved), scene.base, &[(0, 3)]);
    scene.apply(&bytes, on(), start);
    assert!(!scene.scroll.is_active());
}

#[test]
fn a_frame_that_changes_the_geometry_ends_a_running_animation() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let mut moved = panes();
    moved.panes[2].x = 5;
    let bytes = frame(scene.seq, false, Some(moved), scene.base, &[]);
    scene.apply(&bytes, on(), start);
    assert!(!scene.scroll.is_active());
}

#[test]
fn a_repeated_unchanged_geometry_record_does_not_stop_the_animation() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.base -= 3;
    let bytes = frame(scene.seq, false, Some(panes()), scene.base, &[(0, 3)]);
    scene.apply(&bytes, on(), start);
    assert!(scene.scroll.is_active());
}

#[test]
fn a_full_repaint_ends_every_animation() {
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let bytes = frame(scene.seq, true, None, scene.base, &[]);
    scene.apply(&bytes, on(), start);
    assert!(!scene.scroll.is_active());
}

#[test]
fn with_smooth_scrolling_off_nothing_animates() {
    let mut scene = Scene::new();
    let start = Instant::now();
    let off = ScrollSettings {
        enabled: false,
        ..on()
    };
    scene.scroll_with(3, off, start);
    assert!(!scene.scroll.is_active());
}

#[test]
fn a_wheel_notch_and_a_touchpad_scroll_of_three_lines_animate_alike() {
    let start = Instant::now();
    let mut notch = Scene::new();
    notch.scroll(3, start);
    let mut touchpad = Scene::new();
    touchpad.scroll(1, start);
    touchpad.scroll(1, start);
    touchpad.scroll(1, start);
    for step in 0..=10 {
        let at = start + DURATION * step / 10;
        assert_eq!(
            notch.scroll.offset_of(0, at),
            touchpad.scroll.offset_of(0, at)
        );
    }
}

fn cache() -> GlyphCache {
    GlyphCache::new(FontStack::embedded(DEFAULT_FONT_SIZE).expect("no embedded font"))
}

fn layer_at(scene: &mut Scene, cache: &mut GlyphCache, at: Instant) -> ScrollLayer {
    let mut layer = ScrollLayer::default();
    let contrast = Contrast::new(1.0);
    scene.scroll.build_layer(
        &mut layer,
        SceneInputs {
            state: &scene.state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        cache,
        Transparency::OPAQUE,
        &contrast,
        &[],
        at,
    );
    layer
}

fn inside(outer: &PixelRect, inner: &PixelRect) -> bool {
    inner.left >= outer.left
        && inner.right <= outer.right
        && inner.top >= outer.top
        && inner.bottom <= outer.bottom
}

#[test]
fn the_layer_is_trimmed_to_the_pane_and_kept_out_from_under_the_floating_pane() {
    let mut cache = cache();
    let metrics = cache.metrics();
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let layer = layer_at(&mut scene, &mut cache, start + DURATION / 2);
    assert!(!layer.rects.is_empty());
    assert!(!layer.glyphs.is_empty());

    let geometry = panes();
    let content = CellRect::content_of(&geometry.panes[0]).pixels(metrics);
    let floating = CellRect::outer_of(&geometry.panes[2]).pixels(metrics);
    let areas = layer
        .rects
        .iter()
        .map(|rect| area(rect.x, rect.y, rect.width, rect.height))
        .chain(
            layer
                .glyphs
                .iter()
                .chain(layer.color_glyphs.iter())
                .map(|glyph| area(glyph.x, glyph.y, glyph.entry.width, glyph.entry.height)),
        );
    for piece in areas {
        assert!(inside(&content, &piece), "{:?} leaves the pane", piece);
        assert!(
            floating.intersection(&piece).is_none(),
            "{:?} is drawn under the floating pane",
            piece
        );
    }
}

#[test]
fn the_layer_fills_the_rows_the_new_content_has_not_reached_yet() {
    let mut cache = cache();
    let metrics = cache.metrics();
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let layer = layer_at(&mut scene, &mut cache, start);
    let content = CellRect::content_of(&panes().panes[0]).pixels(metrics);
    let bottom_row = PixelRect {
        top: content.bottom - metrics.height as i32,
        ..content
    };
    assert!(
        layer.rects.iter().any(|rect| bottom_row
            .intersection(&area(rect.x, rect.y, rect.width, rect.height))
            .is_some()),
        "the strip must paint the bottom row at the start of the slide"
    );
}

#[test]
fn the_rows_under_an_animation_are_left_to_the_layer() {
    let mut cache = cache();
    let metrics = cache.metrics();
    let mut scene = Scene::new();
    let start = Instant::now();
    scene.scroll(3, start);
    let held = scene.scroll.held_out(&scene.state);
    assert_eq!(held.len(), 1);
    let regions = held[0].regions(metrics);

    let mut retained = RetainedScene::new();
    retained.set_held_out(held);
    retained.refresh(
        SceneInputs {
            state: &scene.state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        &mut cache,
    );
    let mut drawn_elsewhere = 0;
    for row in retained.rows() {
        for piece in row
            .rects
            .iter()
            .map(|rect| area(rect.x, rect.y, rect.width, rect.height))
            .chain(
                row.glyphs
                    .iter()
                    .map(|glyph| area(glyph.x, glyph.y, glyph.entry.width, glyph.entry.height)),
            )
        {
            assert!(
                regions
                    .iter()
                    .all(|region| region.intersection(&piece).is_none()),
                "{:?} was drawn in place under the animation",
                piece
            );
            drawn_elsewhere += 1;
        }
    }
    assert!(drawn_elsewhere > 0, "the other panes must still be drawn");

    retained.set_held_out(Vec::new());
    retained.refresh(
        SceneInputs {
            state: &scene.state,
            phase: BlinkPhase::On,
            paints: &Paints::default(),
            cursor: CursorOptions::default(),
            hovered_link: None,
            preedit: None,
        },
        &mut cache,
    );
    assert_eq!(
        retained.flatten(),
        crate::render::scene::test_helpers::build(&scene.state, &mut cache),
        "ending the animation restores the plain rows"
    );
}

#[test]
fn no_layer_is_built_while_nothing_animates() {
    let mut cache = cache();
    let mut scene = Scene::new();
    let layer = layer_at(&mut scene, &mut cache, Instant::now());
    assert!(layer.is_empty());
    assert!(scene.scroll.held_out(&scene.state).is_empty());
}

#[test]
fn a_glyph_is_trimmed_along_with_its_texture_coordinates() {
    let glyph = GlyphQuad {
        x: 10,
        y: 10,
        entry: crate::render::atlas::AtlasEntry {
            x: 100,
            y: 200,
            width: 8,
            height: 16,
            left: 0,
            top: 0,
        },
        color: [1, 2, 3],
    };
    let region = PixelRect {
        left: 12,
        top: 0,
        right: 100,
        bottom: 20,
    };
    let clipped = clip_glyph(&glyph, -4, &region).unwrap();
    assert_eq!((clipped.x, clipped.y), (12, 6));
    assert_eq!(
        (
            clipped.entry.x,
            clipped.entry.y,
            clipped.entry.width,
            clipped.entry.height
        ),
        (102, 200, 6, 14)
    );
    assert!(clip_glyph(&glyph, 40, &region).is_none());
}
