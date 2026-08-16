use super::*;

#[test]
fn themes_to_string() {
    let fake_config = r#"
        themes {
           dracula {
                fg 248 248 242
                bg 40 42 54
                black 0 0 0
                red 255 85 85
                green 80 250 123
                yellow 241 250 140
                blue 98 114 164
                magenta 255 121 198
                cyan 139 233 253
                white 255 255 255
                orange 255 184 108
            }
        }"#;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config",
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_hex_definitions() {
    let fake_config = r##"
        themes {
            nord {
                fg "#D8DEE9"
                bg "#2E3440"
                black "#3B4252"
                red "#BF616A"
                green "#A3BE8C"
                yellow "#EBCB8B"
                blue "#81A1C1"
                magenta "#B48EAD"
                cyan "#88C0D0"
                white "#E5E9F0"
                orange "#D08770"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_eight_bit_definitions() {
    let fake_config = r##"
        themes {
            default {
                fg 1
                bg 10
                black 20
                red 30
                green 40
                yellow 50
                blue 60
                magenta 70
                cyan 80
                white 90
                orange 254
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_combined_definitions() {
    let fake_config = r##"
        themes {
            default {
                fg 1
                bg 10
                black 20
                red 30
                green 40
                yellow 50
                blue 60
                magenta 70
                cyan 80
                white 255 255 255
                orange "#D08770"
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}

#[test]
fn themes_to_string_with_multiple_theme_definitions() {
    let fake_config = r##"
        themes {
           nord {
               fg "#D8DEE9"
               bg "#2E3440"
               black "#3B4252"
               red "#BF616A"
               green "#A3BE8C"
               yellow "#EBCB8B"
               blue "#81A1C1"
               magenta "#B48EAD"
               cyan "#88C0D0"
               white "#E5E9F0"
               orange "#D08770"
           }
           dracula {
                fg 248 248 242
                bg 40 42 54
                black 0 0 0
                red 255 85 85
                green 80 250 123
                yellow 241 250 140
                blue 98 114 164
                magenta 255 121 198
                cyan 139 233 253
                white 255 255 255
                orange 255 184 108
            }
        }"##;
    let document: KdlDocument = fake_config.parse().unwrap();
    let sourced_from_external_file = false;
    let deserialized =
        Themes::from_kdl(document.get("themes").unwrap(), sourced_from_external_file).unwrap();
    let serialized = Themes::to_kdl(&deserialized).unwrap();
    let deserialized_from_serialized = Themes::from_kdl(
        serialized
            .to_string()
            .parse::<KdlDocument>()
            .unwrap()
            .get("themes")
            .unwrap(),
        sourced_from_external_file,
    )
    .unwrap();
    assert_eq!(
        deserialized, deserialized_from_serialized,
        "Deserialized serialized config equals original config"
    );
    insta::assert_snapshot!(serialized.to_string());
}
