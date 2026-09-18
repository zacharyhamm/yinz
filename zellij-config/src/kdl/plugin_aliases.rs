use super::*;
pub fn from_kdl(kdl_plugin_aliases: &KdlNode) -> Result<PluginAliases, ConfigError> {
    let mut aliases: BTreeMap<String, RunPlugin> = BTreeMap::new();
    if let Some(kdl_plugin_aliases) = kdl_children_nodes!(kdl_plugin_aliases) {
        for alias_definition in kdl_plugin_aliases {
            let alias_name = kdl_name!(alias_definition);
            if let Some(string_url) =
                kdl_get_string_property_or_child_value!(alias_definition, "location")
            {
                let configuration =
                    KdlLayoutParser::parse_plugin_user_configuration(alias_definition)?;
                let initial_cwd = kdl_get_string_property_or_child_value!(alias_definition, "cwd")
                    .map(PathBuf::from);
                let run_plugin = zellij_runtime::plugin_location::run_plugin_from_url(string_url)?
                    .with_configuration(configuration.inner().clone())
                    .with_initial_cwd(initial_cwd);
                aliases.insert(alias_name.to_owned(), run_plugin);
            }
        }
    }
    Ok(PluginAliases { aliases })
}
pub fn to_kdl(value: &PluginAliases, add_comments: bool) -> KdlNode {
    let mut plugins = KdlNode::new("plugins");
    let mut plugins_children = KdlDocument::new();
    for (alias_name, plugin_alias) in value.aliases.iter() {
        let mut plugin_alias_node = KdlNode::new(alias_name.clone());
        let mut plugin_alias_children = KdlDocument::new();
        let location_string = plugin_alias.location.display();

        plugin_alias_node.insert("location", location_string);
        let cwd = plugin_alias.initial_cwd.as_ref();
        let mut has_children = false;
        if let Some(cwd) = cwd {
            has_children = true;
            let mut cwd_node = KdlNode::new("cwd");
            cwd_node.push(cwd.display().to_string());
            plugin_alias_children.nodes_mut().push(cwd_node);
        }
        let configuration = plugin_alias.configuration.inner();
        if !configuration.is_empty() {
            has_children = true;
            for (config_key, config_value) in configuration {
                let mut node = KdlNode::new(config_key.to_owned());
                if config_value == "true" {
                    node.push(KdlValue::Bool(true));
                } else if config_value == "false" {
                    node.push(KdlValue::Bool(false));
                } else {
                    node.push(config_value.to_string());
                }
                plugin_alias_children.nodes_mut().push(node);
            }
        }
        if has_children {
            plugin_alias_node.set_children(plugin_alias_children);
        }
        plugins_children.nodes_mut().push(plugin_alias_node);
    }
    plugins.set_children(plugins_children);

    if add_comments {
        plugins.set_leading(format!(
            "\n{}\n{}\n",
            "// Plugin aliases - can be used to change the implementation of Zellij",
            "// changing these requires a restart to take effect",
        ));
    }
    plugins
}
