use super::*;

/// The top-level config blocks that `Config::from_kdl` reads by name. A second block of the
/// same name in one file used to be silently ignored, because the lookup returned the first.
const BLOCKS: [&str; 8] = [
    "keybinds",
    "themes",
    "plugins",
    "load_plugins",
    "context_menu",
    "hooks",
    "ui",
    "env",
];

fn duplicate_block_error(name: &str) -> (String, KdlError) {
    let document = format!("{name} {{\n}}\n{name} {{\n}}\n");
    match Config::from_kdl(&document, None) {
        Err(ConfigError::KdlError(error)) => (document, error),
        other => panic!("two '{name}' blocks should be a KdlError, got {other:?}"),
    }
}

#[test]
fn a_repeated_top_level_block_is_a_config_error_naming_the_block() {
    for name in BLOCKS {
        let (_, error) = duplicate_block_error(name);
        assert!(
            error.error_message.contains(&format!("'{name}'")),
            "the error for a repeated {name} block should name it, got {:?}",
            error.error_message
        );
        assert!(
            error.error_message.contains("more than once"),
            "the error should say the block is repeated, got {:?}",
            error.error_message
        );
    }
}

#[test]
fn the_error_points_at_the_second_block() {
    for name in BLOCKS {
        let (document, error) = duplicate_block_error(name);
        let parsed: KdlDocument = document.parse().unwrap();
        let second = &parsed.nodes()[1];
        assert_eq!(second.name().value(), name);
        assert_eq!(
            error.offset,
            Some(second.span().offset()),
            "the span for a repeated {name} block should start at the second block"
        );
        assert_eq!(error.len, Some(second.span().len()));
    }
}

#[test]
fn a_single_block_of_each_kind_still_parses() {
    let mut document = String::new();
    for name in BLOCKS {
        document.push_str(&format!("{name} {{\n}}\n"));
    }
    Config::from_kdl(&document, None)
        .unwrap_or_else(|e| panic!("one block of each kind should parse, got {e:?}"));
}

#[test]
fn the_default_config_asset_has_no_repeated_blocks() {
    Config::from_default_assets().expect("the shipped default config should parse");
}

#[test]
fn a_themes_file_with_two_themes_blocks_is_rejected() {
    let document = "themes {\n}\nthemes {\n}\n";
    match Themes::from_string(document, true) {
        Err(ConfigError::KdlError(error)) => {
            assert!(
                error.error_message.contains("'themes'"),
                "{:?}",
                error.error_message
            );
        },
        other => panic!("two themes blocks in a theme file should be rejected, got {other:?}"),
    }
}

#[test]
fn the_same_block_in_a_base_config_and_a_fragment_still_merges() {
    // `config.d` fragments and layouts are separate `from_kdl` calls over the base config;
    // only a repeat *within one document* is an error.
    let base = Config::from_kdl(
        "keybinds {\n normal {\n bind \"Ctrl a\" { NewPane; }\n }\n}\n",
        None,
    )
    .unwrap();
    let merged = Config::from_kdl(
        "keybinds {\n normal {\n bind \"Ctrl b\" { NewTab; }\n }\n}\n",
        Some(base),
    )
    .unwrap();
    let normal = &merged.keybinds.0[&InputMode::Normal];
    assert!(
        normal.contains_key(&KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier()),
        "the base config's binding should survive the fragment"
    );
    assert!(
        normal.contains_key(&KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier()),
        "the fragment's binding should be added"
    );
}
