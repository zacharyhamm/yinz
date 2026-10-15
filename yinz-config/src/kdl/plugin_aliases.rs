use super::*;
pub fn from_kdl(kdl_plugin_aliases: &KdlNode) -> Result<PluginAliases, ConfigError> {
    let mut aliases: BTreeMap<String, RunPlugin> = BTreeMap::new();
    if let Some(kdl_plugin_aliases) = kdl_children_nodes!(kdl_plugin_aliases) {
        for alias_definition in kdl_plugin_aliases {
            let alias_name = kdl_name!(alias_definition);
            use yinz_types::input::{
                git_plugin::{GitPlugin, GitRevision},
                layout::RunPluginLocation,
                plugins::PluginsConfigError,
            };
            let property = |name: &str| -> Result<Option<String>, ConfigError> {
                let entries = alias_definition
                    .entries()
                    .iter()
                    .filter(|entry| entry.name().is_some_and(|key| key.value() == name))
                    .collect::<Vec<_>>();
                let children = alias_definition
                    .children()
                    .map(|children| {
                        children
                            .nodes()
                            .iter()
                            .filter(|node| node.name().value() == name)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if entries.len() + children.len() > 1 {
                    return Err(kdl_parsing_error!(
                        format!("duplicate {name} property"),
                        alias_definition
                    ));
                }
                let entry = entries
                    .first()
                    .copied()
                    .or_else(|| children.first().and_then(|node| node.get(0)));
                if let Some(entry) = entry {
                    return entry
                        .value()
                        .as_string()
                        .map(|s| Some(s.to_owned()))
                        .ok_or_else(|| {
                            kdl_parsing_error!(format!("{name} must be a string"), alias_definition)
                        });
                }
                if !children.is_empty() {
                    return Err(kdl_parsing_error!(
                        format!("{name} requires a string"),
                        alias_definition
                    ));
                }
                Ok(None)
            };
            let repo = property("repo")?;
            let location = property("location")?;
            let entrypoint = property("entrypoint")?;
            let branch = property("branch")?;
            let tag = property("tag")?;
            let commit = property("commit")?;
            if repo.is_some() && location.is_some() {
                return Err(kdl_parsing_error!(
                    "repo and location are mutually exclusive".to_owned(),
                    alias_definition
                ));
            }
            if repo.is_none()
                && [
                    entrypoint.as_ref(),
                    branch.as_ref(),
                    tag.as_ref(),
                    commit.as_ref(),
                ]
                .iter()
                .any(Option::is_some)
            {
                return Err(kdl_parsing_error!(
                    "repository properties require repo".to_owned(),
                    alias_definition
                ));
            }
            if [branch.as_ref(), tag.as_ref(), commit.as_ref()]
                .iter()
                .filter(|value| value.is_some())
                .count()
                > 1
            {
                return Err(kdl_parsing_error!(
                    "branch, tag and commit are mutually exclusive".to_owned(),
                    alias_definition
                ));
            }
            let run_plugin = if let Some(repo) = repo {
                let mut plugin = GitPlugin {
                    repo,
                    entrypoint: entrypoint.unwrap_or_else(|| "main.scm".into()).into(),
                    revision: branch
                        .map(GitRevision::Branch)
                        .or_else(|| tag.map(GitRevision::Tag))
                        .or_else(|| commit.map(GitRevision::Commit)),
                };
                yinz_runtime::plugin_location::resolve_repository_path(
                    &mut plugin,
                    std::path::Path::new(""),
                )?;
                RunPlugin {
                    location: RunPluginLocation::Git(plugin),
                    ..Default::default()
                }
            } else if let Some(location) = location {
                yinz_runtime::plugin_location::run_plugin_from_url(&location)?
            } else {
                return Err(PluginsConfigError::InvalidRepository(format!(
                    "alias {alias_name} requires location or repo"
                ))
                .into());
            };
            let configuration = KdlLayoutParser::parse_plugin_user_configuration(alias_definition)?;
            let initial_cwd = property("cwd")?.map(PathBuf::from);
            aliases.insert(
                alias_name.to_owned(),
                RunPlugin {
                    configuration,
                    initial_cwd,
                    ..run_plugin
                },
            );
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

        if let yinz_types::input::layout::RunPluginLocation::Git(plugin) = &plugin_alias.location {
            plugin_alias_node.insert("repo", plugin.repo.clone());
            plugin_alias_node.insert(
                "entrypoint",
                plugin.entrypoint.to_string_lossy().into_owned(),
            );
            if let Some(revision) = &plugin.revision {
                let (key, value) = revision.property();
                plugin_alias_node.insert(key, value);
            }
        } else {
            plugin_alias_node.insert("location", plugin_alias.location_for_serialization());
        }
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
            "// Plugin aliases - can be used to change the implementation of Yinz",
            "// declarations are refreshed on config reload",
        ));
    }
    plugins
}
