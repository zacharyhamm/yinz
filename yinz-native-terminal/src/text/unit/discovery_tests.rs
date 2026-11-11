use super::*;

const PROBES: [char; 6] = [
    'A',
    '\u{4f60}',
    '\u{1f600}',
    '\u{2801}',
    '\u{e000}',
    '\u{10fffd}',
];

fn carries(path: &std::path::Path, index: u32, character: char) -> Option<bool> {
    let data = std::fs::read(path).ok()?;
    let font = swash::FontRef::from_index(&data, index as usize)?;
    Some(font.charmap().map(character) != 0)
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn a_loader_that_resolves_nothing_falls_back_to_the_embedded_fonts() {
    assert!(
        fontconfig::Fontconfig::load_from("libfontconfig-there-is-no-such-library.so.0").is_none(),
        "a soname that does not resolve produced a working fontconfig"
    );

    let mut fonts =
        crate::text::font::FontStack::embedded(crate::text::font::DEFAULT_FONT_SIZE).unwrap();
    assert!(
        !fonts.has_discovery() && fonts.lookup(FaceStyle::Regular, 'A').is_some(),
        "the embedded fonts stopped covering 'A' without system discovery"
    );
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn a_named_instance_index_addresses_its_face_not_the_instance() {
    use fontconfig::face_index;
    assert_eq!(face_index(0), 0);
    assert_eq!(face_index(3), 3);
    // second named instance of face 0, as fontconfig reports for a variable font
    assert_eq!(face_index(2 << 16), 0);
    assert_eq!(face_index((5 << 16) | 1), 1);
    assert_eq!(face_index(0x8000_0000_u32 as std::ffi::c_int), 0);
    assert_eq!(face_index(0x8000_0001_u32 as std::ffi::c_int), 1);
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn css_weights_map_onto_the_fontconfig_scale() {
    use fontconfig::fontconfig_weight;
    assert_eq!(fontconfig_weight(100), 0);
    assert_eq!(fontconfig_weight(300), 50);
    assert_eq!(fontconfig_weight(400), 80);
    assert_eq!(fontconfig_weight(500), 100);
    assert_eq!(fontconfig_weight(700), 200);
    assert_eq!(fontconfig_weight(900), 210);
    assert_eq!(fontconfig_weight(450), 90);
    assert_eq!(fontconfig_weight(50), 0);
}

#[test]
fn a_matched_font_is_a_readable_file() {
    let Some(discovery) = Discovery::open() else {
        eprintln!("fontconfig is unavailable here; discovery is untested");
        return;
    };
    let Some((path, _index)) = discovery.match_codepoint(FaceStyle::Regular, 'A') else {
        eprintln!("no installed font carries 'A'; discovery is untested");
        return;
    };
    assert!(path.is_file(), "{:?} is not a file", path);
    assert!(std::fs::read(&path).is_ok_and(|data| !data.is_empty()));
}

#[test]
fn a_match_always_carries_the_codepoint_it_was_asked_for() {
    let Some(discovery) = Discovery::open() else {
        return;
    };
    for character in PROBES {
        let Some((path, index)) = discovery.match_codepoint(FaceStyle::Regular, character) else {
            continue;
        };
        assert_ne!(
            carries(&path, index, character),
            Some(false),
            "U+{:04X} was matched to {:?}, which does not carry it",
            character as u32,
            path
        );
    }
}

#[test]
fn opening_discovery_twice_reuses_one_fontconfig() {
    let (Some(first), Some(second)) = (Discovery::open(), Discovery::open()) else {
        return;
    };
    assert!(
        std::ptr::eq(first.0, second.0),
        "fontconfig was loaded and rescanned a second time"
    );
}

fn embedded_font_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/fonts")
}

fn synthetic() -> Option<Discovery> {
    let discovery = Discovery::synthetic(&embedded_font_dir());
    if discovery.is_none() {
        eprintln!("fontconfig is unavailable here; family selection is untested");
    }
    discovery
}

#[test]
fn a_requested_family_resolves_to_a_file_carrying_that_family() {
    let Some(discovery) = synthetic() else {
        return;
    };
    let (path, index) = discovery
        .match_family("Iosevka Term", FaceStyle::Regular)
        .expect("the embedded Iosevka was not found in a config holding only it");
    assert!(path.starts_with(embedded_font_dir()), "{:?}", path);
    let data = std::fs::read(&path).unwrap();
    let font = swash::FontRef::from_index(&data, index as usize).unwrap();
    assert!(
        font.localized_strings()
            .any(|string| string.to_string().eq_ignore_ascii_case("Iosevka Term")),
        "{:?} does not name itself Iosevka Term",
        path
    );
}

#[test]
fn the_installed_monospaced_families_are_listed_once_each() {
    let Some(discovery) = synthetic() else {
        return;
    };
    let families = discovery.monospace_families();
    assert_eq!(
        families
            .iter()
            .filter(|family| family.eq_ignore_ascii_case("Iosevka Term"))
            .count(),
        1,
        "{:?}",
        families
    );
}

#[test]
fn the_colour_emoji_font_is_found_for_an_emoji() {
    let Some(discovery) = synthetic() else {
        return;
    };
    let (path, _) = discovery
        .match_emoji('\u{1f600}')
        .expect("the colour emoji font was not found in a config holding it");
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("NotoColorEmoji.ttf")
    );
}

#[test]
fn a_family_that_is_not_installed_resolves_to_nothing() {
    let Some(discovery) = synthetic() else {
        return;
    };
    assert_eq!(
        discovery.match_family("No Such Family At All", FaceStyle::Regular),
        None,
        "fontconfig's best guess was taken for the family that was asked for"
    );
}

#[test]
fn a_requested_family_answers_every_style() {
    let Some(discovery) = synthetic() else {
        return;
    };
    for style in [
        FaceStyle::Regular,
        FaceStyle::Bold,
        FaceStyle::Italic,
        FaceStyle::BoldItalic,
    ] {
        assert!(
            discovery.match_family("Iosevka Term", style).is_some(),
            "{:?} did not resolve",
            style
        );
    }
}

#[test]
fn every_style_is_matched_without_crashing() {
    let Some(discovery) = Discovery::open() else {
        return;
    };
    for style in [
        FaceStyle::Regular,
        FaceStyle::Bold,
        FaceStyle::Italic,
        FaceStyle::BoldItalic,
    ] {
        if let Some((path, index)) = discovery.match_codepoint(style, 'x') {
            assert_ne!(
                carries(&path, index, 'x'),
                Some(false),
                "{:?} was matched to {:?}, which has no 'x'",
                style,
                path
            );
        }
    }
}
