//! The colour ladder in `frame_color`, and in particular where the related-pane tint sits
//! in it.
//!
//! The tint is the gentlest state there is, and every one of these tests exists because the
//! cheap way to write the branch - putting it higher in the ladder, or giving it a
//! precedence somebody else already has - is wrong in a way no compiler would catch.

use super::*;
use yinz_types::data::{Palette, Styling};

const CLIENT: ClientId = ClientId::new(1);

/// The default theme, which is the one that matters: it leaves `frame_unselected` as `None`,
/// so an ordinary pane's frame has no colour at all and the tint has nothing to be a
/// variation of.
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
        frame_color_override: None,
        waiting_pulse: None,
        pane_is_focused: false,
        pane_is_in_group: false,
        pane_is_related: false,
        mouse_is_hovering_over_pane: false,
        multiple_users_exist_in_session: false,
    }
}

fn color_of(style: &Style, state: FrameColorState) -> Option<(PaletteColor, usize)> {
    frame_color(style, state, CLIENT, InputMode::Normal, false)
}

#[test]
fn a_related_pane_is_coloured_where_an_ordinary_one_is_not() {
    let style = default_style();
    assert_eq!(
        color_of(&style, state()),
        None,
        "the default theme leaves an ordinary frame with no colour - if this changes, the \
         tint below is no longer distinguishable from doing nothing"
    );
    let (color, precedence) = color_of(
        &style,
        FrameColorState {
            pane_is_related: true,
            ..state()
        },
    )
    .expect("a related pane is drawn in some colour");
    assert_eq!(
        precedence, 2,
        "related leaves room for an idle waiting pulse below it"
    );
    assert_ne!(
        color, style.colors.frame_selected.base,
        "faded, not the focus colour itself: a related pane must not read as a second focus"
    );
}

/// The tint is derived from two colours every theme defines, so it does not quietly vanish
/// on a theme that names one for ordinary frames.
#[test]
fn the_tint_does_not_depend_on_frame_unselected() {
    let bare = color_of(
        &default_style(),
        FrameColorState {
            pane_is_related: true,
            ..state()
        },
    );
    let themed = color_of(
        &style_with_frame_unselected(),
        FrameColorState {
            pane_is_related: true,
            ..state()
        },
    );
    assert_eq!(bare, themed);
    assert!(bare.is_some());
}

#[test]
fn focus_beats_related() {
    let style = default_style();
    assert_eq!(
        color_of(
            &style,
            FrameColorState {
                pane_is_related: true,
                pane_is_focused: true,
                ..state()
            }
        ),
        color_of(
            &style,
            FrameColorState {
                pane_is_focused: true,
                ..state()
            }
        ),
        "the focused pane is the one everything else is kin *of*; it keeps its own colour"
    );
}

#[test]
fn a_group_beats_related() {
    let style = default_style();
    assert_eq!(
        color_of(
            &style,
            FrameColorState {
                pane_is_related: true,
                pane_is_in_group: true,
                ..state()
            }
        ),
        color_of(
            &style,
            FrameColorState {
                pane_is_in_group: true,
                ..state()
            }
        ),
        "a group is something the user asked for; an ambient hint must not paint over it"
    );
}

#[test]
fn hover_beats_related() {
    let style = default_style();
    assert_eq!(
        color_of(
            &style,
            FrameColorState {
                pane_is_related: true,
                mouse_is_hovering_over_pane: true,
                ..state()
            }
        ),
        color_of(
            &style,
            FrameColorState {
                mouse_is_hovering_over_pane: true,
                ..state()
            }
        ),
    );
}

/// Where two panes share one boundary cell, `combine_symbols` settles it on the precedence
/// alone - and with `>=`, so a tie goes to whichever pane's `add_rect` ran first, which is
/// pane id order and means nothing to whoever is looking at the screen. No two states that
/// can apply to two neighbours at once may share a number.
#[test]
fn every_ambient_state_has_a_precedence_of_its_own() {
    let style = default_style();
    let precedence = |state| color_of(&style, state).map(|(_, precedence)| precedence);
    let related = precedence(FrameColorState {
        pane_is_related: true,
        ..state()
    });
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
        (related, hovered, grouped, focused),
        (Some(2), Some(4), Some(6), Some(8)),
        "nothing in particular < related < hovered < grouped < focused"
    );
}

/// A bell on a related pane eases back to the *tint*, not to nothing - the override branch
/// recomputes the settled colour with the same state, so it has to carry `pane_is_related`
/// through. Getting that wrong makes a bell wipe the highlight off for as long as it lasts.
#[test]
fn a_fading_override_settles_back_onto_the_tint() {
    let style = default_style();
    let tint = color_of(
        &style,
        FrameColorState {
            pane_is_related: true,
            ..state()
        },
    )
    .map(|(color, _)| color);
    let settled = color_of(
        &style,
        FrameColorState {
            pane_is_related: true,
            // a flash that has finished fading contributes nothing
            frame_color_override: Some((style.colors.exit_code_error.base, 0.0)),
            ..state()
        },
    )
    .map(|(color, _)| color);
    assert_eq!(settled, tint);
}

#[test]
fn an_override_at_full_strength_wins_outright() {
    let style = default_style();
    let (color, precedence) = color_of(
        &style,
        FrameColorState {
            pane_is_related: true,
            frame_color_override: Some((style.colors.exit_code_error.base, 1.0)),
            ..state()
        },
    )
    .expect("an override is always a colour");
    assert_eq!(color, style.colors.exit_code_error.base);
    assert_eq!(precedence, 10);
}

#[test]
fn waiting_pulse_brightens_the_resolved_color_and_breaks_base_priority_ties() {
    let foreground = PaletteColor::Rgb((100, 150, 200));
    let mut indexed = style_with_frame_unselected();
    indexed.colors.frame_unselected.as_mut().unwrap().base = PaletteColor::EightBit(24);
    for style in [default_style(), indexed] {
        for flags in 0..16 {
            let make_state = || FrameColorState {
                pane_is_focused: flags & 1 != 0,
                pane_is_in_group: flags & 2 != 0,
                pane_is_related: flags & 4 != 0,
                mouse_is_hovering_over_pane: flags & 8 != 0,
                ..state()
            };
            let base = color_of(&style, make_state());
            assert_eq!(
                color_of(
                    &style,
                    FrameColorState {
                        waiting_pulse: Some((0.0, foreground)),
                        ..make_state()
                    }
                ),
                base
            );
            let (color, precedence) = base.unwrap_or((foreground, 0));
            assert_eq!(
                color_of(
                    &style,
                    FrameColorState {
                        waiting_pulse: Some((0.4, foreground)),
                        ..make_state()
                    }
                ),
                Some((
                    blend(color, PaletteColor::Rgb((255, 255, 255)), 0.4),
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
                        waiting_pulse: Some((0.4, foreground)),
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
                pane_is_related: true,
                ..state()
            },
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
    for (index, base) in states().into_iter().take(5).enumerate() {
        let pulsed = color_of(
            &style,
            FrameColorState {
                waiting_pulse: Some((0.4, PaletteColor::Rgb((100, 150, 200)))),
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
