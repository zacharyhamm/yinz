//! The colour ladder in `frame_color`: which state wins, and the precedence it wins with.

use super::*;
use yinz_types::data::{Palette, Styling};

/// The default theme, which leaves `frame_unselected` as `None`, so an ordinary pane's frame
/// has no colour at all.
fn default_style() -> Style {
    Style::default()
}

/// A theme that does name a colour for an ordinary frame, so the branch is exercised both
/// ways round.
fn style_with_frame_unselected() -> Style {
    let mut style = Style::default();
    style.colors.frame_unselected = Some(Styling::from(Palette::default()).text_unselected);
    style
}

fn state() -> FrameColorState {
    FrameColorState {
        host_colors: (
            PaletteColor::Rgb((100, 150, 200)),
            PaletteColor::Rgb((20, 30, 40)),
        ),
        frame_color_override: None,
        waiting_pulse: None,
        pane_is_focused: false,
        pane_is_in_group: false,
        mouse_is_hovering_over_pane: false,
        multiple_users_exist_in_session: false,
        color_slot: ColorSlot::new(1),
    }
}

fn color_of(style: &Style, state: FrameColorState) -> Option<(PaletteColor, usize)> {
    frame_color(style, state, InputMode::Normal, false)
}

#[test]
fn hover_uses_the_tab_button_color_without_changing_focus_or_group_colors() {
    let mut style = default_style();
    style.colors.ribbon_unselected.emphasis_1 = PaletteColor::Rgb((12, 34, 56));
    style.colors.frame_selected.base = PaletteColor::Rgb((78, 90, 123));
    style.colors.frame_highlight.base = PaletteColor::Rgb((145, 167, 189));
    for mode in [InputMode::Normal, InputMode::Locked, InputMode::Pane] {
        for (focused, grouped, expected) in [
            (false, false, (style.colors.ribbon_unselected.emphasis_1, 4)),
            (false, true, (style.colors.frame_highlight.emphasis_0, 6)),
            (true, true, (style.colors.frame_highlight.emphasis_1, 8)),
            (
                true,
                false,
                (
                    if matches!(mode, InputMode::Normal | InputMode::Locked) {
                        style.colors.frame_selected.base
                    } else {
                        style.colors.frame_highlight.base
                    },
                    8,
                ),
            ),
        ] {
            assert_eq!(
                frame_color(
                    &style,
                    FrameColorState {
                        mouse_is_hovering_over_pane: true,
                        pane_is_focused: focused,
                        pane_is_in_group: grouped,
                        ..state()
                    },
                    mode,
                    false,
                ),
                Some(expected),
            );
        }
    }
}

#[test]
fn a_hovered_panes_override_fades_back_to_the_tab_button_color() {
    let mut style = default_style();
    style.colors.ribbon_unselected.emphasis_1 = PaletteColor::Rgb((12, 34, 56));
    let override_color = PaletteColor::Rgb((210, 180, 150));
    for strength in [0.0, 0.5, 1.0] {
        assert_eq!(
            color_of(
                &style,
                FrameColorState {
                    mouse_is_hovering_over_pane: true,
                    frame_color_override: Some((override_color, strength)),
                    ..state()
                },
            ),
            Some((
                blend(
                    style.colors.ribbon_unselected.emphasis_1,
                    override_color,
                    strength
                ),
                if strength == 0.0 { 4 } else { 10 },
            )),
        );
    }
}

/// Where two panes share one boundary cell, `combine_symbols` settles it on the precedence
/// alone - and with `>=`, so a tie goes to whichever pane's `add_rect` ran first, which is
/// pane id order and means nothing to whoever is looking at the screen. No two states that
/// can apply to two neighbours at once may share a number.
#[test]
fn every_ambient_state_has_a_precedence_of_its_own() {
    let style = default_style();
    let precedence = |state| color_of(&style, state).map(|(_, precedence)| precedence);
    let hovered = precedence(FrameColorState {
        mouse_is_hovering_over_pane: true,
        ..state()
    });
    let grouped = precedence(FrameColorState {
        pane_is_in_group: true,
        ..state()
    });
    let focused = precedence(FrameColorState {
        pane_is_focused: true,
        ..state()
    });
    assert_eq!(
        (hovered, grouped, focused),
        (Some(4), Some(6), Some(8)),
        "nothing in particular < hovered < grouped < focused"
    );
}

#[test]
fn an_override_at_full_strength_wins_outright() {
    let style = default_style();
    let (color, precedence) = color_of(
        &style,
        FrameColorState {
            frame_color_override: Some((style.colors.exit_code_error.base, 1.0)),
            ..state()
        },
    )
    .expect("an override is always a colour");
    assert_eq!(color, style.colors.exit_code_error.base);
    assert_eq!(precedence, 10);
}

#[test]
fn waiting_pulse_uses_chrome_foreground_and_breaks_base_priority_ties() {
    let mut indexed = style_with_frame_unselected();
    indexed.colors.frame_unselected.as_mut().unwrap().base = PaletteColor::EightBit(24);
    for style in [default_style(), indexed] {
        for flags in 0..8 {
            let make_state = || FrameColorState {
                pane_is_focused: flags & 1 != 0,
                pane_is_in_group: flags & 2 != 0,
                mouse_is_hovering_over_pane: flags & 4 != 0,
                ..state()
            };
            let base = color_of(&style, make_state());
            assert_eq!(
                color_of(
                    &style,
                    FrameColorState {
                        waiting_pulse: Some(0.0),
                        ..make_state()
                    }
                ),
                base
            );
            let (color, precedence) = base.unwrap_or((state().host_colors.0, 0));
            assert_eq!(
                color_of(
                    &style,
                    FrameColorState {
                        waiting_pulse: Some(0.4),
                        ..make_state()
                    }
                ),
                Some((
                    blend(
                        color,
                        if color == style.colors.text_unselected.base {
                            style.colors.frame_selected.base
                        } else {
                            style.colors.text_unselected.base
                        },
                        0.4,
                    ),
                    precedence + 1
                ))
            );
        }
        // Errors, explicit highlights and fading terminal bells all use this override slot.
        for strength in [0.0, 0.3, 1.0] {
            let override_ = Some((PaletteColor::EightBit(1), strength));
            assert_eq!(
                color_of(
                    &style,
                    FrameColorState {
                        frame_color_override: override_,
                        waiting_pulse: Some(0.4),
                        ..state()
                    }
                ),
                color_of(
                    &style,
                    FrameColorState {
                        frame_color_override: override_,
                        ..state()
                    }
                ),
            );
        }
    }
}

#[test]
fn waiting_pulse_loses_to_every_higher_base_state() {
    let style = style_with_frame_unselected();
    let states = || {
        [
            state(),
            FrameColorState {
                mouse_is_hovering_over_pane: true,
                ..state()
            },
            FrameColorState {
                pane_is_in_group: true,
                ..state()
            },
            FrameColorState {
                pane_is_focused: true,
                ..state()
            },
            FrameColorState {
                frame_color_override: Some((style.colors.exit_code_error.base, 1.0)),
                ..state()
            },
        ]
    };
    for (index, base) in states().into_iter().take(4).enumerate() {
        let pulsed = color_of(
            &style,
            FrameColorState {
                waiting_pulse: Some(0.4),
                ..base
            },
        )
        .unwrap()
        .1;
        for higher in states().into_iter().skip(index + 1) {
            assert!(pulsed < color_of(&style, higher).unwrap().1);
        }
    }
}

#[test]
fn waiting_pulses_stay_theme_colored_on_opposite_host_palettes() {
    for (background, foreground) in [
        ((245, 245, 245), (35, 35, 35)),
        ((25, 25, 25), (220, 220, 220)),
    ] {
        let mut style = default_style();
        style.terminal = Some(yinz_types::style::TerminalTheme {
            foreground,
            background,
            ansi: [(0, 0, 0); 16],
        });
        style.colors.text_unselected.base = PaletteColor::Rgb(foreground);
        style.colors.frame_selected.base = PaletteColor::Rgb((120, 90, 160));
        for focused in [false, true] {
            let normal = color_of(
                &style,
                FrameColorState {
                    pane_is_focused: focused,
                    ..state()
                },
            )
            .map(|(color, _)| color)
            .unwrap_or(style.colors.text_unselected.base);
            let pulse = |host| {
                color_of(
                    &style,
                    FrameColorState {
                        pane_is_focused: focused,
                        host_colors: (host, host),
                        waiting_pulse: Some(0.4),
                        ..state()
                    },
                )
                .unwrap()
                .0
            };
            let light_host = pulse(PaletteColor::Rgb((0, 0, 0)));
            assert_eq!(light_host, pulse(PaletteColor::Rgb((255, 255, 255))));
            assert_ne!(light_host, normal);
        }
    }
}

#[test]
fn unfocused_bell_fades_to_the_ordinary_frame_color() {
    let override_color = PaletteColor::Rgb((240, 50, 20));
    for terminal in [
        None,
        Some(yinz_types::style::TerminalTheme {
            foreground: (230, 240, 250),
            background: (20, 30, 40),
            ansi: [(0, 0, 0); 16],
        }),
    ] {
        for unselected in [None, style_with_frame_unselected().colors.frame_unselected] {
            let mut style = default_style();
            style.terminal = terminal;
            style.colors.frame_unselected = unselected;
            let normal = color_of(&style, state());
            let foreground = unselected
                .map(|frame| frame.base)
                .unwrap_or(style.chrome_colors().unwrap_or(state().host_colors).0);
            for strength in [0.0, 0.5, 1.0] {
                assert_eq!(
                    color_of(
                        &style,
                        FrameColorState {
                            frame_color_override: Some((override_color, strength)),
                            ..state()
                        }
                    ),
                    if strength == 0.0 {
                        normal
                    } else {
                        Some((blend(foreground, override_color, strength), 10))
                    }
                );
            }
        }
    }
}

#[test]
fn coincident_pulse_targets_blend_toward_the_farther_black_or_white() {
    use PaletteColor::{EightBit, Rgb};
    let indexed_rgb = Rgb(yinz_types::colors::eightbit_to_rgb(245));
    for (start, target, endpoint) in [
        (Rgb((0, 0, 0)), Rgb((0, 0, 0)), Rgb((255, 255, 255))),
        (Rgb((255, 255, 255)), Rgb((255, 255, 255)), Rgb((0, 0, 0))),
        (
            Rgb((127, 127, 128)),
            Rgb((127, 127, 128)),
            Rgb((255, 255, 255)),
        ),
        (Rgb((127, 128, 128)), Rgb((127, 128, 128)), Rgb((0, 0, 0))),
        (indexed_rgb, EightBit(245), Rgb((0, 0, 0))),
        (EightBit(245), indexed_rgb, Rgb((0, 0, 0))),
    ] {
        let mut style = default_style();
        style.colors.text_unselected.base = target;
        style.colors.frame_selected.base = target;
        assert_eq!(
            color_of(
                &style,
                FrameColorState {
                    host_colors: (start, state().host_colors.1),
                    waiting_pulse: Some(0.7),
                    ..state()
                }
            ),
            Some((blend(start, endpoint, 0.7), 1))
        );
    }
}

#[test]
fn unthemed_pulses_start_at_the_host_foreground() {
    let style = default_style();
    for foreground in [
        PaletteColor::Rgb((0, 0, 0)),
        PaletteColor::Rgb((255, 255, 255)),
    ] {
        assert_eq!(
            color_of(
                &style,
                FrameColorState {
                    host_colors: (foreground, state().host_colors.1),
                    waiting_pulse: Some(0.4),
                    ..state()
                }
            ),
            Some((blend(foreground, style.colors.text_unselected.base, 0.4), 1))
        );
    }
}

#[test]
fn a_focused_frame_wears_the_viewers_color_slot_when_several_clients_share_the_session() {
    let mut style = default_style();
    style.colors.multiplayer_user_colors.player_7 = PaletteColor::Rgb((7, 7, 7));
    let focused = |color_slot, multiple_users_exist_in_session| {
        color_of(
            &style,
            FrameColorState {
                pane_is_focused: true,
                multiple_users_exist_in_session,
                color_slot,
                ..state()
            },
        )
    };
    assert_eq!(
        focused(ColorSlot::new(7), true),
        Some((PaletteColor::Rgb((7, 7, 7)), 8))
    );
    assert_eq!(
        focused(None, true),
        None,
        "past ten clients there is no color"
    );
    assert_eq!(
        focused(ColorSlot::new(7), false),
        Some((style.colors.frame_selected.base, 8)),
        "a lone client draws in the selected-frame color"
    );
}
