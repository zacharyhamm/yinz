use super::*;

fn resolved(root: &Path, theme: &str) -> AgentThemeSnapshot {
    write(
        root,
        ".claude.json",
        &serde_json::json!({"theme": theme}).to_string(),
    );
    let mut source = snapshot();
    source.provider = "claude".into();
    claude::resolve(&context(root), &mut source).unwrap();
    source.finish();
    source
}

fn definition(source: &AgentThemeSnapshot, token: &str, color: SourceColor, role: Role) {
    let channel = match role {
        Role::AdditionBackground
        | Role::RemovalBackground
        | Role::AdditionWord
        | Role::RemovalWord
        | Role::AdditionDimmed
        | Role::RemovalDimmed => Channel::Background,
        _ => Channel::Foreground,
    };
    assert!(
        source.definitions.iter().any(|d| d.token == token
            && d.color == color
            && d.channel == channel
            && d.target == Some(role.into())),
        "{}: {token} {color:?} {role:?}",
        source.theme
    );
}

#[test]
fn presets_and_auto_match_the_installed_code_renderer() {
    use Role::*;
    use SourceColor::{Index, Rgb};
    let dir = tempfile::tempdir().unwrap();
    // Independent Q/N/fe outputs from the pinned binary, including its special dark fills.
    for (theme, colors, indexes) in [
        (
            "light",
            [
                (220, 255, 220),
                (178, 255, 178),
                (36, 138, 61),
                (255, 220, 220),
                (255, 199, 199),
                (207, 34, 46),
                (51, 51, 51),
            ],
            [194, 157, 29, 224, 224, 160, 236],
        ),
        (
            "dark",
            [
                (2, 40, 0),
                (4, 71, 0),
                (80, 200, 80),
                (61, 1, 0),
                (92, 2, 0),
                (220, 90, 90),
                (248, 248, 242),
            ],
            [22, 28, 77, 52, 52, 167, 231],
        ),
        (
            "light-daltonized",
            [
                (219, 237, 255),
                (179, 217, 255),
                (36, 87, 138),
                (255, 220, 220),
                (255, 199, 199),
                (207, 34, 46),
                (51, 51, 51),
            ],
            [195, 153, 24, 224, 224, 160, 236],
        ),
        (
            "dark-daltonized",
            [
                (0, 27, 41),
                (0, 48, 71),
                (81, 160, 200),
                (61, 1, 0),
                (92, 2, 0),
                (220, 90, 90),
                (248, 248, 242),
            ],
            [17, 24, 74, 52, 52, 167, 231],
        ),
    ] {
        let source = resolved(dir.path(), theme);
        let auto = resolved(dir.path(), "auto");
        for ((token, role), ((r, g, b), index)) in [
            ("addLine", AdditionBackground),
            ("addWord", AdditionWord),
            ("addDecoration", Success),
            ("deleteLine", RemovalBackground),
            ("deleteWord", RemovalWord),
            ("deleteDecoration", Error),
            ("foreground", Foreground),
        ]
        .into_iter()
        .zip(colors.into_iter().zip(indexes))
        {
            let token = format!("code.{token}");
            definition(&source, &token, Rgb(r, g, b), role);
            definition(&source, &format!("{token}.index"), Index(index), role);
            if !theme.contains("daltonized") {
                definition(&auto, &token, Rgb(r, g, b), role);
                definition(&auto, &format!("{token}.index"), Index(index), role);
            }
        }
    }
    for theme in ["light-ansi", "dark-ansi"] {
        let source = resolved(dir.path(), theme);
        definition(
            &source,
            "code.foreground",
            Index(if theme == "dark-ansi" { 7 } else { 0 }),
            Foreground,
        );
        definition(&source, "code.addDecoration", Index(10), Success);
        definition(&source, "code.deleteDecoration", Index(9), Error);
        assert!(!source
            .definitions
            .iter()
            .any(|d| d.token == "code.addLine" || d.token == "code.deleteLine"));
        assert_eq!(
            source.compile().0[&MatchKey {
                channel: Channel::Background,
                color: SourceColor::Default,
                pair: None
            }],
            Some(Background.into())
        );
    }
}

#[test]
fn syntax_scopes_match_verified_light_dark_and_ansi_colors() {
    use Role::*;
    let dir = tempfile::tempdir().unwrap();
    // Scope colors and indexed emissions were read from G/P/he and N in the renderer,
    // not from Yinz's descriptor or conversion helper.
    let cases = [
        (
            "keyword|operator",
            Accent,
            (167, 29, 93),
            125,
            (249, 38, 114),
            197,
        ),
        ("_storage", Accent, (167, 29, 93), 125, (102, 217, 239), 81),
        (
            "built_in|type|attr|attribute",
            Accent,
            (0, 134, 179),
            31,
            (166, 226, 46),
            148,
        ),
        (
            "literal|number|symbol",
            Warning,
            (0, 134, 179),
            31,
            (190, 132, 255),
            141,
        ),
        (
            "string|regexp",
            Success,
            (24, 54, 145),
            24,
            (230, 219, 116),
            186,
        ),
        (
            "title|title.function",
            Accent,
            (121, 93, 163),
            97,
            (166, 226, 46),
            148,
        ),
        (
            "title.class|title.class.inherited",
            Accent,
            (0, 0, 0),
            16,
            (166, 226, 46),
            148,
        ),
        ("params", Accent, (0, 134, 179), 31, (253, 151, 31), 208),
        (
            "comment|meta",
            Muted,
            (150, 152, 150),
            246,
            (117, 113, 94),
            242,
        ),
        (
            "variable|variable.language|property",
            Accent,
            (0, 134, 179),
            31,
            (255, 255, 255),
            231,
        ),
        (
            "punctuation|subst",
            Foreground,
            (51, 51, 51),
            236,
            (248, 248, 242),
            231,
        ),
    ];
    for theme in [
        "light",
        "dark",
        "light-daltonized",
        "dark-daltonized",
        "auto",
    ] {
        let source = resolved(dir.path(), theme);
        for (tokens, role, light, light_index, dark, dark_index) in cases {
            for (rgb, index) in if theme == "auto" {
                vec![(light, light_index), (dark, dark_index)]
            } else if theme.starts_with("dark") {
                vec![(dark, dark_index)]
            } else {
                vec![(light, light_index)]
            } {
                for token in tokens.split('|') {
                    definition(
                        &source,
                        &format!("code.{token}"),
                        SourceColor::Rgb(rgb.0, rgb.1, rgb.2),
                        role,
                    );
                    definition(
                        &source,
                        &format!("code.{token}.index"),
                        SourceColor::Index(index),
                        role,
                    );
                }
            }
        }
    }
    for theme in ["light-ansi", "dark-ansi"] {
        let source = resolved(dir.path(), theme);
        for (tokens, index, role) in [
            ("keyword", 13, Accent),
            ("_storage|built_in|type", 14, Accent),
            ("literal|number", 12, Warning),
            ("string", 10, Success),
            ("title|title.function|title.class", 11, Accent),
            ("comment|meta", 8, Muted),
        ] {
            for token in tokens.split('|') {
                definition(
                    &source,
                    &format!("code.{token}"),
                    SourceColor::Index(index),
                    role,
                );
            }
        }
    }
}

#[test]
fn custom_diff_overrides_replace_code_fills_and_quantize_without_changing_syntax() {
    use Role::*;
    use SourceColor::{Index, Rgb};
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        ".claude/themes/test.json",
        r##"{"base":"dark","overrides":{
        "diffAdded":"#123456", "diffRemoved":"ansi256(123)",
        "diffAddedDimmed":"#999", "diffRemovedDimmed":"ansi:red",
        "diffAddedWord":"rgb(100,120,140)", "diffRemovedWord":"ansi:blueBright",
        "code.keyword":"#abcdef"
    }}"##,
    );
    let source = resolved(dir.path(), "custom:test");
    for (token, rgb, indexed, role) in [
        ("diffAdded", Rgb(18, 52, 86), Index(23), AdditionBackground),
        ("diffRemoved", Index(123), Index(123), RemovalBackground),
        (
            "diffAddedDimmed",
            Rgb(153, 153, 153),
            Index(247),
            AdditionDimmed,
        ),
        ("diffRemovedDimmed", Index(1), Index(1), RemovalDimmed),
        ("diffAddedWord", Rgb(100, 120, 140), Index(66), AdditionWord),
        ("diffRemovedWord", Index(12), Index(12), RemovalWord),
    ] {
        definition(&source, &format!("code.{token}"), rgb, role);
        definition(
            &source,
            &format!(
                "code.{token}{}",
                if matches!(rgb, Rgb(..)) { ".index" } else { "" }
            ),
            indexed,
            role,
        );
    }
    assert!(!source.definitions.iter().any(|d| matches!(
        d.token.as_str(),
        "code.addLine"
            | "code.addLine.index"
            | "code.deleteLine"
            | "code.addWord"
            | "code.addWord.index"
            | "code.deleteWord"
    )));
    definition(&source, "code.keyword", Rgb(249, 38, 114), Accent);
    definition(&source, "code.addDecoration", Rgb(80, 200, 80), Success);

    // A dimmed-only override leaves the ordinary fill in use; an invalid override is ignored.
    write(
        dir.path(),
        ".claude/themes/test.json",
        r#"{"base":"dark","overrides":{"diffAddedDimmed":"ansi256(123)","diffRemoved":"invalid"}}"#,
    );
    let source = resolved(dir.path(), "custom:test");
    definition(&source, "code.addLine", Rgb(2, 40, 0), AdditionBackground);
    definition(&source, "code.deleteLine", Rgb(61, 1, 0), RemovalBackground);
}

#[test]
fn overlapping_code_colors_keep_existing_collision_precedence() {
    use Role::*;
    use SourceColor::{Index, Rgb};
    let dir = tempfile::tempdir().unwrap();
    for (theme, channel, color, role, tokens) in [
        (
            "light",
            Channel::Foreground,
            Rgb(0, 134, 179),
            Accent,
            ["code.type", "code.number"],
        ),
        (
            "light",
            Channel::Foreground,
            Rgb(0, 0, 0),
            Foreground,
            ["code.title.class", "text"],
        ),
        (
            "dark",
            Channel::Foreground,
            Rgb(255, 255, 255),
            Foreground,
            ["code.variable", "text"],
        ),
        (
            "dark",
            Channel::Foreground,
            Index(231),
            Foreground,
            ["code.variable.index", "code.foreground.index"],
        ),
        (
            "light",
            Channel::Background,
            Index(224),
            RemovalBackground,
            ["code.deleteLine.index", "code.deleteWord.index"],
        ),
        (
            "dark",
            Channel::Background,
            Index(52),
            RemovalBackground,
            ["code.deleteLine.index", "code.deleteWord.index"],
        ),
        (
            "light-ansi",
            Channel::Foreground,
            Index(12),
            Accent,
            ["code.number", "ide"],
        ),
        (
            "dark-ansi",
            Channel::Foreground,
            Index(9),
            Accent,
            ["code.deleteDecoration", "claude"],
        ),
    ] {
        let mut source = resolved(dir.path(), theme);
        let key = MatchKey {
            channel,
            color,
            pair: None,
        };
        let compiled = source.compile();
        assert_eq!(compiled.0[&key], Some(role.into()), "{theme} {key:?}");
        assert!(source
            .collisions
            .iter()
            .any(|c| c.key == key && tokens.iter().all(|t| c.tokens.iter().any(|v| v == t))));
        source.definitions.reverse();
        assert_eq!(source.compile(), compiled);
    }
}

#[test]
fn code_quantizer_matches_verified_cube_gray_and_boundary_outputs() {
    for (rgb, index) in [
        ((0, 0, 0), 16),
        ((4, 4, 4), 16),
        ((5, 5, 5), 232),
        ((8, 8, 8), 232),
        ((244, 244, 244), 255),
        ((245, 245, 245), 231),
        ((255, 255, 255), 231),
        ((47, 0, 0), 233),
        ((48, 0, 0), 233),
        ((114, 0, 0), 52),
        ((115, 0, 0), 88),
        ((2, 40, 0), 233),
        ((0, 27, 41), 234),
        ((51, 51, 51), 236),
        ((150, 152, 150), 246),
        ((230, 219, 116), 186),
    ] {
        assert_eq!(
            claude::code_color_index(rgb.0, rgb.1, rgb.2),
            index,
            "{rgb:?}"
        );
    }
}

#[test]
fn render_fixture_uses_resolved_definitions_and_collision_results() {
    let dir = tempfile::tempdir().unwrap();
    let source = resolved(dir.path(), "light");
    let fixture: AgentThemeSnapshot =
        serde_json::from_str(include_str!("fixtures/claude-light-code.json")).unwrap();
    for definition in &fixture.definitions {
        assert!(source.definitions.contains(definition), "{definition:?}");
    }
    let compiled = source.compile().0;
    for (key, target) in fixture.compile().0 {
        assert_eq!(compiled[&key], target, "{key:?}");
    }
}
