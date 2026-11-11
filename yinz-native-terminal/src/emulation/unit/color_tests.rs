use super::test_helpers::*;
use super::*;

fn paints() -> Paints {
    Paints::default()
}

#[test]
fn the_extended_palette_matches_the_xterm_table() {
    assert_eq!(indexed(16), [0, 0, 0]);
    assert_eq!(indexed(196), [255, 0, 0]);
    assert_eq!(indexed(231), [255, 255, 255]);
    assert_eq!(indexed(232), [8, 8, 8]);
    assert_eq!(indexed(255), [238, 238, 238]);
}

#[test]
fn the_low_palette_is_the_same_table_the_wire_seed_uses() {
    for index in 0u8..16 {
        assert_eq!(indexed(index), ANSI_16[index as usize]);
    }
}

#[test]
fn the_wire_default_is_the_foreground_or_the_background_by_position() {
    let default = WireColor::Default.pack();
    assert_eq!(paints().foreground(default), DEFAULT_FOREGROUND);
    assert_eq!(paints().background(default), DEFAULT_BACKGROUND);
}

#[test]
fn every_wire_color_kind_resolves_to_its_table_entry() {
    let paints = paints();
    assert_eq!(paints.foreground(WireColor::Named(1).pack()), ANSI_16[1]);
    assert_eq!(paints.foreground(WireColor::Named(14).pack()), ANSI_16[14]);
    assert_eq!(
        paints.foreground(WireColor::Indexed(208).pack()),
        indexed(208)
    );
    assert_eq!(paints.foreground(WireColor::Rgb(9, 8, 7).pack()), [9, 8, 7]);
    assert_eq!(paints.background(WireColor::Rgb(9, 8, 7).pack()), [9, 8, 7]);
}

#[test]
fn a_configured_table_answers_named_and_indexed_colors_alike() {
    let mut paints = paints();
    paints.ansi[1] = [1, 2, 3];
    assert_eq!(paints.foreground(WireColor::Named(1).pack()), [1, 2, 3]);
    assert_eq!(paints.foreground(WireColor::Indexed(1).pack()), [1, 2, 3]);
    assert_eq!(
        paints.foreground(WireColor::Indexed(208).pack()),
        indexed(208),
        "the extended palette is not affected by the low table"
    );
}

#[test]
fn configured_defaults_answer_the_wire_default() {
    let paints = Paints {
        foreground: [1, 1, 1],
        background: [2, 2, 2],
        ..Paints::default()
    };
    let default = WireColor::Default.pack();
    assert_eq!(paints.foreground(default), [1, 1, 1]);
    assert_eq!(paints.background(default), [2, 2, 2]);
}

#[test]
fn an_unconfigured_cursor_takes_the_color_it_is_drawn_over() {
    let paints = paints();
    assert_eq!(paints.cursor_over([1, 2, 3]), [1, 2, 3]);
    assert_eq!(paints.beam_cursor(), DEFAULT_CURSOR);
}

#[test]
fn a_configured_cursor_is_used_whatever_it_is_drawn_over() {
    let paints = Paints {
        cursor: Some([9, 9, 9]),
        ..Paints::default()
    };
    assert_eq!(paints.cursor_over([1, 2, 3]), [9, 9, 9]);
    assert_eq!(paints.beam_cursor(), [9, 9, 9]);
}

#[test]
fn contrast_ratios_match_the_wcag_reference_values() {
    assert!((contrast_ratio(WHITE, BLACK) - 21.0).abs() < 1e-9);
    assert!((contrast_ratio(BLACK, WHITE) - 21.0).abs() < 1e-9);
    assert!((contrast_ratio([120, 30, 200], [120, 30, 200]) - 1.0).abs() < 1e-9);
    assert!((contrast_ratio([0x77, 0x77, 0x77], WHITE) - 4.48).abs() < 0.01);
    assert!((contrast_ratio([0xff, 0x00, 0x00], WHITE) - 4.0).abs() < 0.01);
    assert!((relative_luminance([0x80, 0x80, 0x80]) - 0.2159).abs() < 0.001);
}

#[test]
fn low_contrast_text_is_moved_just_far_enough_to_reach_the_target() {
    for (foreground, background, minimum) in [
        ([60, 60, 60], [40, 40, 40], 4.5),
        ([200, 200, 210], [230, 230, 230], 3.0),
        ([0, 0, 238], [0, 0, 0], 7.0),
        ([128, 128, 128], [128, 128, 128], 4.5),
        ([10, 10, 10], [118, 118, 118], 7.0),
    ] {
        let adjusted = with_minimum_contrast(foreground, background, minimum);
        let ratio = contrast_ratio(adjusted, background);
        let best = contrast_ratio(WHITE, background).max(contrast_ratio(BLACK, background));
        assert!(
            ratio >= minimum || adjusted == WHITE || adjusted == BLACK,
            "{:?} on {:?} became {:?} at {}",
            foreground,
            background,
            adjusted,
            ratio
        );
        if best >= minimum {
            assert!(ratio >= minimum, "{:?} on {:?}", foreground, background);
        }
    }
}

#[test]
fn the_smaller_move_wins_and_unreachable_targets_take_the_better_extreme() {
    let lightened = with_minimum_contrast([90, 90, 90], [60, 60, 60], 3.0);
    assert!(lightened[0] > 90, "{:?}", lightened);
    let darkened = with_minimum_contrast([170, 170, 170], [200, 200, 200], 3.0);
    assert!(darkened[0] < 170, "{:?}", darkened);
    let unreachable = with_minimum_contrast([118, 118, 118], [118, 118, 118], 21.0);
    let mid = [118, 118, 118];
    let expected = if contrast_ratio(WHITE, mid) >= contrast_ratio(BLACK, mid) {
        WHITE
    } else {
        BLACK
    };
    assert_eq!(unreachable, expected);
}

#[test]
fn high_contrast_text_and_an_inactive_minimum_change_nothing() {
    assert_eq!(
        with_minimum_contrast([229, 229, 229], BLACK, 4.5),
        [229, 229, 229]
    );
    assert_eq!(
        with_minimum_contrast([40, 40, 40], BLACK, 1.0),
        [40, 40, 40]
    );
    let off = Contrast::new(1.0);
    assert_eq!(off.apply([40, 40, 40], BLACK), [40, 40, 40]);
    assert_eq!(off.cached_pairs(), 0);
}

#[test]
fn adjusted_pairs_are_cached() {
    let contrast = Contrast::new(4.5);
    let first = contrast.apply([40, 40, 40], BLACK);
    assert_eq!(contrast.cached_pairs(), 1);
    assert_eq!(contrast.apply([40, 40, 40], BLACK), first);
    assert_eq!(contrast.cached_pairs(), 1);
}

#[test]
fn dimming_darkens_by_a_third() {
    assert_eq!(dim([255, 255, 255]), [170, 170, 170]);
    assert_eq!(dim([0, 0, 0]), [0, 0, 0]);
}
