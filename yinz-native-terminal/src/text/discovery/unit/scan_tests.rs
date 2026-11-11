use super::*;
use crate::text::font::FaceStyle;

fn face(family: &str, monospaced: bool, bold: bool, italic: bool) -> Face {
    Face {
        families: vec![family.to_owned()],
        monospaced,
        weight: if bold { 700 } else { 400 },
        italic,
        width: NORMAL_WIDTH,
    }
}

fn weighted(family: &str, weight: u16) -> Face {
    Face {
        families: vec![family.to_owned()],
        monospaced: true,
        weight,
        italic: false,
        width: NORMAL_WIDTH,
    }
}

#[test]
fn a_family_prefers_its_normal_width_to_wider_and_narrower_faces() {
    let wide = Face {
        width: 6,
        ..weighted("Mono", 400)
    };
    let narrow = Face {
        width: 3,
        ..weighted("Mono", 400)
    };
    let wide_bold = Face {
        width: 6,
        ..weighted("Mono", 700)
    };
    let faces = vec![
        wide,
        narrow,
        wide_bold,
        weighted("Mono", 400),
        weighted("Mono", 700),
    ];
    assert_eq!(family_choice(&faces, "Mono", upright(400)), Some(3));
    assert_eq!(
        family_choice(&faces, "Mono", FaceStyle::Bold.into()),
        Some(4)
    );
}

#[test]
fn width_only_breaks_ties_between_faces_of_the_requested_style() {
    let wide_italic = Face {
        width: 6,
        ..face("Mono", true, false, true)
    };
    let condensed_bold = Face {
        width: 4,
        ..face("Mono", true, true, false)
    };
    let faces = vec![
        face("Mono", true, false, false),
        wide_italic,
        condensed_bold,
    ];
    assert_eq!(
        family_choice(&faces, "Mono", FaceStyle::Italic.into()),
        Some(1),
        "an italic in another width still beats an upright face"
    );
    assert_eq!(
        family_choice(&faces, "Mono", FaceStyle::Bold.into()),
        Some(2),
        "a bold in another width still beats a regular face"
    );
}

fn upright(weight: u16) -> FaceRequest {
    FaceRequest {
        weight,
        italic: false,
    }
}

fn weights() -> Vec<Face> {
    vec![
        weighted("Mono", 100),
        weighted("Mono", 300),
        weighted("Mono", 400),
        weighted("Mono", 500),
        weighted("Mono", 700),
    ]
}

#[test]
fn colour_emoji_families_are_ordered_by_preference_and_others_left_out() {
    let faces = vec![
        face("Noto Color Emoji", false, false, false),
        face("DejaVu Sans", false, false, false),
        face("Apple Color Emoji", false, false, false),
        face("Symbola", false, false, false),
    ];
    assert_eq!(emoji_order(&faces), vec![2, 0]);
    assert!(emoji_order(&[face("Symbola", false, false, false)]).is_empty());
}

#[test]
fn only_monospaced_families_are_listed_once_each_in_order() {
    let faces = vec![
        face("Zed Mono", true, false, false),
        face("Arial", false, false, false),
        face("iosevka term", true, false, false),
        face("Iosevka Term", true, true, false),
        face(".Hidden Mono", true, false, false),
    ];
    assert_eq!(
        monospace_families(&faces),
        vec!["iosevka term".to_owned(), "Zed Mono".to_owned()]
    );
}

#[test]
fn the_default_request_picks_regular_over_thin_light_and_medium() {
    assert_eq!(
        family_choice(&weights(), "Mono", FaceStyle::Regular.into()),
        Some(2)
    );
    let mut shuffled = weights();
    shuffled.rotate_left(3);
    assert_eq!(
        shuffled[family_choice(&shuffled, "Mono", FaceStyle::Regular.into()).unwrap()].weight,
        400
    );
}

#[test]
fn a_medium_request_picks_medium() {
    assert_eq!(family_choice(&weights(), "Mono", upright(500)), Some(3));
}

#[test]
fn bold_picks_the_heavier_face() {
    assert_eq!(
        family_choice(&weights(), "Mono", FaceStyle::Bold.into()),
        Some(4)
    );
    assert_eq!(
        family_choice(
            &weights(),
            "Mono",
            FaceRequest::weighted(FaceStyle::Bold, 500)
        ),
        Some(4)
    );
}

#[test]
fn ties_go_heavier_from_regular_up_and_lighter_below_it() {
    let faces = vec![
        weighted("Mono", 300),
        weighted("Mono", 500),
        weighted("Mono", 200),
        weighted("Mono", 400),
    ];
    assert_eq!(family_choice(&faces, "Mono", upright(450)), Some(1));
    assert_eq!(family_choice(&faces, "Mono", upright(250)), Some(2));
    assert_eq!(
        family_choice(&faces[..2], "Mono", upright(400)),
        Some(1),
        "with no exact match a regular request leans heavier"
    );
    assert_eq!(
        family_choice(&faces[..3], "Mono", upright(250)),
        Some(2),
        "below regular a request leans lighter"
    );
}

#[test]
fn a_light_request_takes_the_nearest_face_and_the_lighter_of_two_equals() {
    let faces = vec![weighted("Mono", 400), weighted("Mono", 100)];
    assert_eq!(family_choice(&faces, "Mono", upright(300)), Some(0));
    let faces = vec![weighted("Mono", 400), weighted("Mono", 200)];
    assert_eq!(family_choice(&faces, "Mono", upright(300)), Some(1));
}

#[test]
fn preferred_families_come_first_in_the_order_they_are_listed() {
    let faces = vec![
        face("Some Sans", false, false, false),
        face("Emoji", false, false, false),
        face("Mono", true, false, false),
        face("CJK", false, false, false),
    ];
    assert_eq!(
        fallback_order(&faces, &["mono", "CJK", "Emoji"], FaceStyle::Regular.into()),
        vec![2, 3, 1, 0]
    );
}

#[test]
fn among_the_rest_monospaced_faces_come_before_proportional_ones() {
    let faces = vec![
        face("Proportional", false, false, false),
        face("Fixed", true, false, false),
        face("Other Proportional", false, false, false),
        face("Other Fixed", true, false, false),
    ];
    assert_eq!(
        fallback_order(&faces, &[], FaceStyle::Regular.into()),
        vec![1, 3, 0, 2]
    );
}

#[test]
fn within_a_family_the_requested_style_wins_and_weight_matters_more_than_slant() {
    let faces = vec![
        face("Mono", true, false, false),
        face("Mono", true, true, false),
        face("Mono", true, false, true),
        face("Mono", true, true, true),
    ];
    assert_eq!(
        fallback_order(&faces, &["Mono"], FaceStyle::Bold.into()),
        vec![1, 3, 0, 2]
    );
    assert_eq!(
        fallback_order(&faces, &["Mono"], FaceStyle::Italic.into()),
        vec![2, 0, 3, 1]
    );
}

#[test]
fn a_family_is_found_by_name_regardless_of_case_and_its_closest_style_is_chosen() {
    let faces = vec![
        face("Other", true, true, false),
        face("Iosevka Term", true, false, false),
        face("Iosevka Term", true, true, false),
    ];
    assert_eq!(
        family_choice(&faces, "iosevka term", FaceStyle::Bold.into()),
        Some(2)
    );
    assert_eq!(
        family_choice(&faces, "Iosevka Term", FaceStyle::BoldItalic.into()),
        Some(2)
    );
    assert_eq!(
        family_choice(&faces, "Iosevka Term", FaceStyle::Italic.into()),
        Some(1)
    );
    assert_eq!(
        family_choice(&faces, "Missing", FaceStyle::Regular.into()),
        None
    );
}

#[test]
fn each_desktop_platform_names_monospace_cjk_and_emoji_fallbacks_and_linux_defers_to_fontconfig() {
    assert!(preferred_fallbacks(Platform::Linux).is_empty());
    for (platform, monospace, cjk, emoji) in [
        (Platform::MacOs, "Menlo", "PingFang SC", "Apple Color Emoji"),
        (
            Platform::Windows,
            "Consolas",
            "Microsoft YaHei",
            "Segoe UI Emoji",
        ),
    ] {
        let preferred = preferred_fallbacks(platform);
        for family in [monospace, cjk, emoji] {
            assert!(preferred.contains(&family), "{:?} {}", platform, family);
        }
        let position = |family| preferred.iter().position(|name| *name == family);
        assert!(position(monospace) < position(cjk));
        assert!(position(cjk) < position(emoji));
    }
}
