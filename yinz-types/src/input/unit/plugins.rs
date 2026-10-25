use super::*;
use crate::input::layout::PluginAlias;

#[test]
fn presentation_uses_the_resolved_location_not_the_alias_name() {
    for (location, tab_bar, opening_ripple) in [
        (
            RunPluginLocation::Yinz(PluginTag::new("tab-bar")),
            true,
            false,
        ),
        (
            RunPluginLocation::Yinz(PluginTag::new("agents")),
            false,
            true,
        ),
        (
            RunPluginLocation::Yinz(PluginTag::new("status-bar")),
            false,
            false,
        ),
        (RunPluginLocation::File("tab-bar".into()), false, false),
        (RunPluginLocation::File("agents".into()), false, false),
    ] {
        let plugin = RunPlugin {
            location,
            ..Default::default()
        };
        let expected = PluginPresentation {
            tab_bar,
            opening_ripple,
        };
        assert_eq!(
            RunPluginOrAlias::RunPlugin(plugin.clone()).presentation(),
            expected
        );
        for name in ["tab-bar", "agents", "custom-alias"] {
            let mut alias = RunPluginOrAlias::Alias(PluginAlias::new(name, &None, None));
            assert_eq!(alias.presentation(), PluginPresentation::default());
            alias.populate_run_plugin_if_needed(&PluginAliases::from_data(
                [(name.to_owned(), plugin.clone())].into(),
            ));
            assert_eq!(alias.presentation(), expected);
        }
    }
}
