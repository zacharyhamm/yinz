use super::*;
use yinz_types::input::cli_assets::CliAssets;

#[test]
fn migration_preserves_default_tab_and_constrained_swap_layouts() {
    let layout = yinz_config::kdl::layout::from_kdl(
        r#"
        layout {
            default_tab_template { pane name="custom"; children; }
            tab name="existing" { pane; }
            swap_tiled_layout name="custom tiled" {
                tab max_panes=3 { pane split_direction="vertical" { pane; pane; }; }
            }
            swap_floating_layout name="custom floating" {
                floating_panes max_panes=2 { pane x=1 y=2 width=30 height=10; }
            }
        }
    "#,
        None,
        None,
        None,
    )
    .unwrap();
    let assets = CliAssets {
        update_layout: Some(serde_json::to_string(&MigrationLayout::from(layout.clone())).unwrap()),
        ..Default::default()
    };
    let restored = replacement_layout(&assets).unwrap();
    assert_eq!(restored.new_tab(), layout.new_tab());
    assert_eq!(restored, layout);
    assert!(!restored.swap_tiled_layouts.is_empty());
    assert!(!restored.swap_floating_layouts.is_empty());
}

#[test]
fn migration_requires_valid_effective_layout() {
    assert!(replacement_layout(&CliAssets::default()).is_err());
    assert!(replacement_layout(&CliAssets {
        update_layout: Some("invalid".into()),
        ..Default::default()
    })
    .is_err());
}
