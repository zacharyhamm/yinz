use super::*;
use crate::config::NotWritableIn;
fn bind_keys_in_block(
    block: &KdlNode,
    input_mode_keybinds: &mut HashMap<KeyWithModifier, Vec<Action>>,
    config_options: &Options,
) -> Result<(), ConfigError> {
    let all_nodes = kdl_children_nodes_or_error!(block, "no keybinding block for mode");
    let bind_nodes = all_nodes.iter().filter(|n| kdl_name!(n) == "bind");
    let unbind_nodes = all_nodes.iter().filter(|n| kdl_name!(n) == "unbind");
    for key_block in bind_nodes {
        crate::kdl::keybinds::bind_actions_for_each_key(
            key_block,
            input_mode_keybinds,
            config_options,
        )?;
    }
    // we loop a second time so that the unbinds always happen after the binds
    for key_block in unbind_nodes {
        crate::kdl::keybinds::unbind_keys(key_block, input_mode_keybinds)?;
    }
    for key_block in all_nodes {
        if kdl_name!(key_block) != "bind" && kdl_name!(key_block) != "unbind" {
            return Err(ConfigError::new_kdl_error(
                format!("Unknown keybind instruction: '{}'", kdl_name!(key_block)),
                key_block.span().offset(),
                key_block.span().len(),
            ));
        }
    }
    Ok(())
}
pub fn from_kdl(
    kdl_keybinds: &KdlNode,
    base_keybinds: Keybinds,
    config_options: &Options,
) -> Result<Keybinds, ConfigError> {
    let clear_defaults = kdl_arg_is_truthy!(kdl_keybinds, "clear-defaults");
    let mut keybinds_from_config = if clear_defaults {
        Keybinds::default()
    } else {
        base_keybinds
    };
    let mut global_unbind_blocks = vec![];
    for block in kdl_children_nodes_or_error!(kdl_keybinds, "keybindings with no children") {
        let mut block_without_source;
        let source = crate::editor::keybind_block_source(block).map_err(|message| {
            ConfigError::new_kdl_error(message, block.span().offset(), block.span().len())
        })?;
        if source
            .as_ref()
            .is_some_and(|source| source.name().value() == "unbind")
        {
            global_unbind_blocks.push(block);
            continue;
        }
        let block = if source.is_some() {
            block_without_source = block.clone();
            block_without_source.entries_mut().retain(|entry| {
                entry
                    .name()
                    .is_none_or(|name| name.value() != crate::editor::KEYBIND_BLOCK_SOURCE)
            });
            &block_without_source
        } else {
            block
        };
        if kdl_name!(block) == "shared_except" || kdl_name!(block) == "shared" {
            let mut modes_to_exclude = vec![];
            for mode_name in kdl_string_arguments!(block) {
                modes_to_exclude.push(InputMode::from_str(mode_name).map_err(|_| {
                    ConfigError::new_kdl_error(
                        format!("Invalid mode: '{}'", mode_name),
                        block.name().span().offset(),
                        block.name().span().len(),
                    )
                })?);
            }
            for mode in InputMode::iter() {
                if modes_to_exclude.contains(&mode) {
                    continue;
                }
                let input_mode_keybinds = keybinds_from_config.get_input_mode_mut(&mode);
                crate::kdl::keybinds::bind_keys_in_block(
                    block,
                    input_mode_keybinds,
                    config_options,
                )?;
            }
        }
        if kdl_name!(block) == "shared_among" {
            let mut modes_to_include = vec![];
            for mode_name in kdl_string_arguments!(block) {
                modes_to_include.push(InputMode::from_str(mode_name)?);
            }
            for mode in InputMode::iter() {
                if !modes_to_include.contains(&mode) {
                    continue;
                }
                let input_mode_keybinds = keybinds_from_config.get_input_mode_mut(&mode);
                crate::kdl::keybinds::bind_keys_in_block(
                    block,
                    input_mode_keybinds,
                    config_options,
                )?;
            }
        }
    }
    for mode in kdl_children_nodes_or_error!(kdl_keybinds, "keybindings with no children") {
        if kdl_name!(mode) == "unbind"
            || kdl_name!(mode) == "shared_except"
            || kdl_name!(mode) == "shared_among"
            || kdl_name!(mode) == "shared"
            || mode.get(crate::editor::KEYBIND_BLOCK_SOURCE).is_some()
        {
            continue;
        }
        let input_mode_keybinds =
            crate::kdl::keybinds::input_mode_keybindings(mode, &mut keybinds_from_config)?;
        crate::kdl::keybinds::bind_keys_in_block(mode, input_mode_keybinds, config_options)?;
    }
    for block in global_unbind_blocks {
        let input_mode_keybinds =
            crate::kdl::keybinds::input_mode_keybindings(block, &mut keybinds_from_config)?;
        crate::kdl::keybinds::bind_keys_in_block(block, input_mode_keybinds, config_options)?;
    }
    if let Some(global_unbind) = kdl_keybinds.children().and_then(|c| c.get("unbind")) {
        crate::kdl::keybinds::unbind_keys_in_all_modes(global_unbind, &mut keybinds_from_config)?;
    };
    Ok(keybinds_from_config)
}
fn bind_actions_for_each_key(
    key_block: &KdlNode,
    input_mode_keybinds: &mut HashMap<KeyWithModifier, Vec<Action>>,
    config_options: &Options,
) -> Result<(), ConfigError> {
    let keys: Vec<KeyWithModifier> = keys_from_kdl!(key_block);
    let actions: Vec<Action> = actions_from_kdl!(key_block, config_options);
    for key in keys {
        input_mode_keybinds.insert(key, actions.clone());
    }
    Ok(())
}
fn unbind_keys(
    key_block: &KdlNode,
    input_mode_keybinds: &mut HashMap<KeyWithModifier, Vec<Action>>,
) -> Result<(), ConfigError> {
    let keys: Vec<KeyWithModifier> = keys_from_kdl!(key_block);
    for key in keys {
        input_mode_keybinds.remove(&key);
    }
    Ok(())
}
fn unbind_keys_in_all_modes(
    global_unbind: &KdlNode,
    keybinds_from_config: &mut Keybinds,
) -> Result<(), ConfigError> {
    let keys: Vec<KeyWithModifier> = keys_from_kdl!(global_unbind);
    for mode in keybinds_from_config.0.values_mut() {
        for key in &keys {
            mode.remove(key);
        }
    }
    Ok(())
}
fn input_mode_keybindings<'a>(
    mode: &KdlNode,
    keybinds_from_config: &'a mut Keybinds,
) -> Result<&'a mut HashMap<KeyWithModifier, Vec<Action>>, ConfigError> {
    let mode_name = kdl_name!(mode);
    let input_mode = InputMode::from_str(mode_name).map_err(|_| {
        ConfigError::new_kdl_error(
            format!("Invalid mode: '{}'", mode_name),
            mode.name().span().offset(),
            mode.name().span().len(),
        )
    })?;
    let input_mode_keybinds = keybinds_from_config.get_input_mode_mut(&input_mode);
    let clear_defaults_for_mode = kdl_arg_is_truthy!(mode, "clear-defaults");
    if clear_defaults_for_mode {
        input_mode_keybinds.clear();
    }
    Ok(input_mode_keybinds)
}
pub fn from_string(
    stringified_keybindings: String,
    base_keybinds: Keybinds,
    config_options: &Options,
) -> Result<Keybinds, ConfigError> {
    let document: KdlDocument = stringified_keybindings.parse()?;
    if let Some(kdl_keybinds) = document.get("keybinds") {
        crate::kdl::keybinds::from_kdl(kdl_keybinds, base_keybinds, config_options)
    } else {
        Err(ConfigError::new_kdl_error(
            "Could not find keybinds node".to_string(),
            document.span().offset(),
            document.span().len(),
        ))
    }
}
// minimize keybind entries for serialization, so that duplicate entries will appear in
// "shared" nodes later rather than once per mode
fn minimize_entries(
    value: &Keybinds,
) -> BTreeMap<BTreeSet<InputMode>, BTreeMap<KeyWithModifier, Vec<Action>>> {
    let mut minimized: BTreeMap<BTreeSet<InputMode>, BTreeMap<KeyWithModifier, Vec<Action>>> =
        BTreeMap::new();
    let mut flattened: Vec<BTreeMap<KeyWithModifier, Vec<Action>>> = value
        .0
        .values()
        .map(|keybind| keybind.clone().into_iter().collect())
        .collect();
    for keybind in flattened.drain(..) {
        for (key, actions) in keybind.into_iter() {
            let mut appears_in_modes: BTreeSet<InputMode> = BTreeSet::new();
            for (input_mode, keybinds) in value.0.iter() {
                if keybinds.get(&key) == Some(&actions) {
                    appears_in_modes.insert(*input_mode);
                }
            }
            minimized
                .entry(appears_in_modes)
                .or_default()
                .insert(key, actions);
        }
    }
    minimized
}
fn serialize_mode_title_node(_value: &Keybinds, input_modes: &BTreeSet<InputMode>) -> KdlNode {
    let all_modes: Vec<InputMode> = InputMode::iter().collect();
    let total_input_mode_count = all_modes.len();
    if input_modes.len() == 1 {
        let input_mode_name = format!("{:?}", input_modes.iter().next().unwrap()).to_lowercase();
        KdlNode::new(input_mode_name)
    } else if input_modes.len() == total_input_mode_count {
        KdlNode::new("shared")
    } else if input_modes.len() < total_input_mode_count / 2 {
        let mut node = KdlNode::new("shared_among");
        for input_mode in input_modes {
            node.push(format!("{:?}", input_mode).to_lowercase());
        }
        node
    } else {
        let mut node = KdlNode::new("shared_except");
        let mut modes = all_modes.clone();
        for input_mode in input_modes {
            modes.retain(|m| m != input_mode)
        }
        for mode in modes {
            node.push(format!("{:?}", mode).to_lowercase());
        }
        node
    }
}
/// The `bind` lines of one mode block. An action with no KDL spelling is an error naming the
/// binding it sits in, never a line quietly left out: a binding that vanishes on save is a
/// binding the user loses.
fn serialize_mode_keybinds(
    input_modes: &BTreeSet<InputMode>,
    keybinds: &BTreeMap<KeyWithModifier, Vec<Action>>,
) -> Result<KdlDocument, NotWritableIn> {
    let mut mode_keybinds = KdlDocument::new();
    for keybind in keybinds {
        let mut keybind_node = KdlNode::new("bind");
        keybind_node.push(keybind.0.to_keybind_string());
        let mut actions = KdlDocument::new();
        let mut actions_have_children = false;
        for (index, action) in keybind.1.iter().enumerate() {
            let kdl_action = action::to_kdl(action).map_err(|source| NotWritableIn::Keybind {
                modes: input_modes
                    .iter()
                    .map(|mode| format!("{:?}", mode).to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", "),
                key: keybind.0.to_keybind_string(),
                index,
                source,
            })?;
            if kdl_action.children().is_some() {
                actions_have_children = true;
            }
            actions.nodes_mut().push(kdl_action);
        }
        if !actions_have_children {
            for action in actions.nodes_mut() {
                action.set_leading("");
                action.set_trailing("; ");
            }
            actions.set_leading(" ");
            actions.set_trailing("");
        }
        keybind_node.set_children(actions);
        mode_keybinds.nodes_mut().push(keybind_node);
    }
    Ok(mode_keybinds)
}
/// Editor paths must not move when bindings become shared with another mode.
pub fn to_editor_kdl(value: &Keybinds) -> Result<KdlNode, NotWritableIn> {
    let mut root = KdlNode::new("keybinds");
    root.insert("clear-defaults", true);
    let modes: BTreeMap<_, _> = value.0.iter().collect();
    for (mode, bindings) in modes {
        let mut node = KdlNode::new(format!("{mode:?}").to_lowercase());
        node.set_children(serialize_mode_keybinds(
            &BTreeSet::from([*mode]),
            &bindings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )?);
        root.ensure_children().nodes_mut().push(node);
    }
    root.ensure_children();
    Ok(root)
}

pub fn to_kdl(value: &Keybinds, should_clear_defaults: bool) -> Result<KdlNode, NotWritableIn> {
    let mut keybinds_node = KdlNode::new("keybinds");
    if should_clear_defaults {
        keybinds_node.insert("clear-defaults", true);
    }
    let mut minimized = minimize_entries(value);
    let mut keybinds_children = KdlDocument::new();

    macro_rules! encode_single_input_mode {
        ($mode_name:ident) => {{
            let input_modes = BTreeSet::from([InputMode::$mode_name]);
            if let Some(keybinds) = minimized.remove(&input_modes) {
                let mut mode_node =
                    KdlNode::new(format!("{:?}", InputMode::$mode_name).to_lowercase());
                let mode_keybinds = serialize_mode_keybinds(&input_modes, &keybinds)?;
                mode_node.set_children(mode_keybinds);
                keybinds_children.nodes_mut().push(mode_node);
            }
        }};
    }
    // we do this explicitly so that the sorting order of modes in the config is more Human
    // readable - this is actually less code (and clearer) than implementing Ord in this case
    encode_single_input_mode!(Normal);
    encode_single_input_mode!(Locked);
    encode_single_input_mode!(Pane);
    encode_single_input_mode!(Tab);
    encode_single_input_mode!(Resize);
    encode_single_input_mode!(Move);
    encode_single_input_mode!(Scroll);
    encode_single_input_mode!(Search);
    encode_single_input_mode!(Session);
    encode_single_input_mode!(Agent);
    encode_single_input_mode!(Notifications);

    for (input_modes, keybinds) in minimized {
        if input_modes.is_empty() {
            log::error!("invalid input mode for keybinds: {:#?}", keybinds);
            continue;
        }
        let mut mode_node = serialize_mode_title_node(value, &input_modes);
        let mode_keybinds = serialize_mode_keybinds(&input_modes, &keybinds)?;
        mode_node.set_children(mode_keybinds);
        keybinds_children.nodes_mut().push(mode_node);
    }
    keybinds_node.set_children(keybinds_children);
    Ok(keybinds_node)
}
