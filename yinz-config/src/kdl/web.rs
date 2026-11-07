//! The `web { }` block; see `yinz_types::input::web`.
//!
//! `WebConfig` lives in `yinz-types`, so its KDL spelling is a pair of functions here rather
//! than inherent methods.

use super::*;
use crate::kdl_parsing_error;
use yinz_types::input::web::WebConfig;

const BOOLEANS: [&str; 2] = ["enabled", "allow_non_loopback"];
const STRINGS: [&str; 2] = ["address", "plugin"];
const LISTS: [&str; 3] = ["allowed_hosts", "allowed_origins", "tailscale_users"];
const NUMBERS: [&str; 3] = ["port", "session_days", "https_port_hint"];

/// Parse one `web { }` block. Validation that depends on the merged result (the loopback rule,
/// the `session_days` range) is `WebConfig::validate`, which `Config::from_kdl` runs after
/// merging.
pub(crate) fn from_kdl(block: &KdlNode) -> Result<WebConfig, ConfigError> {
    if !block.entries().is_empty() {
        return Err(kdl_parsing_error!(
            "web accepts only child entries, eg. `web { enabled true }`".to_owned(),
            block
        ));
    }
    let mut web = WebConfig::default();
    let Some(children) = block.children() else {
        return Ok(web);
    };
    // Scalar settings seen so far; a repeat is an error rather than a silent override. List
    // settings may repeat, each line adding values.
    let mut seen = std::collections::HashSet::new();
    for node in children.nodes() {
        let name = node.name().value();
        if node.children().is_some() || node.entries().iter().any(|entry| entry.name().is_some()) {
            return Err(kdl_parsing_error!(
                format!("web {name} takes plain values only"),
                node
            ));
        }
        let values: Vec<&KdlValue> = node.entries().iter().map(|entry| entry.value()).collect();
        if LISTS.contains(&name) {
            let strings = values
                .iter()
                .map(|value| {
                    value.as_string().map(str::to_owned).ok_or_else(|| {
                        kdl_parsing_error!(format!("web {name} takes strings"), node)
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let list = match name {
                "allowed_hosts" => &mut web.allowed_hosts,
                "allowed_origins" => &mut web.allowed_origins,
                _ => &mut web.tailscale_users,
            };
            list.extend(strings);
            continue;
        }
        let scalar = BOOLEANS.iter().chain(&STRINGS).chain(&NUMBERS);
        if scalar.into_iter().any(|scalar| *scalar == name) && !seen.insert(name) {
            return Err(kdl_parsing_error!(format!("web {name} is set twice"), node));
        }
        let [value] = values.as_slice() else {
            return Err(kdl_parsing_error!(
                format!("web {name} takes exactly one value"),
                node
            ));
        };
        if BOOLEANS.contains(&name) {
            let flag = value.as_bool().ok_or_else(|| {
                kdl_parsing_error!(format!("web {name} takes true or false"), node)
            })?;
            match name {
                "enabled" => web.enabled = Some(flag),
                _ => web.allow_non_loopback = Some(flag),
            }
        } else if STRINGS.contains(&name) {
            let text = value
                .as_string()
                .ok_or_else(|| kdl_parsing_error!(format!("web {name} takes a string"), node))?
                .to_owned();
            match name {
                "address" => web.address = Some(text),
                _ => web.plugin = Some(text),
            }
        } else if NUMBERS.contains(&name) {
            let number = value.as_i64().ok_or_else(|| {
                kdl_parsing_error!(format!("web {name} takes a whole number"), node)
            })?;
            match name {
                "session_days" => {
                    web.session_days = Some(u32::try_from(number).map_err(|_| {
                        kdl_parsing_error!(
                            format!("web session_days {number} is out of range"),
                            node
                        )
                    })?)
                },
                _ => {
                    let port = u16::try_from(number).map_err(|_| {
                        kdl_parsing_error!(format!("web {name} {number} is not a port"), node)
                    })?;
                    match name {
                        "port" => web.port = Some(port),
                        _ => web.https_port_hint = Some(port),
                    }
                },
            }
        } else {
            return Err(kdl_parsing_error!(
                format!(
                    "unknown web setting '{name}'; known settings: {}",
                    BOOLEANS
                        .iter()
                        .chain(&STRINGS)
                        .chain(&NUMBERS)
                        .chain(&LISTS)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                node
            ));
        }
    }
    Ok(web)
}

/// The block, or `None` when it equals the default. Live update rebuilds the config from this
/// text, so every set field must survive the round trip.
pub(crate) fn to_kdl(web: &WebConfig) -> Option<KdlNode> {
    if *web == WebConfig::default() {
        return None;
    }
    let mut children = KdlDocument::new();
    let mut push = |name: &str, values: Vec<KdlValue>| {
        let mut node = KdlNode::new(name);
        for value in values {
            node.push(value);
        }
        children.nodes_mut().push(node);
    };
    if let Some(enabled) = web.enabled {
        push("enabled", vec![KdlValue::Bool(enabled)]);
    }
    if let Some(address) = &web.address {
        push("address", vec![KdlValue::String(address.clone())]);
    }
    if let Some(port) = web.port {
        push("port", vec![KdlValue::Base10(i64::from(port))]);
    }
    if let Some(allow) = web.allow_non_loopback {
        push("allow_non_loopback", vec![KdlValue::Bool(allow)]);
    }
    if let Some(plugin) = &web.plugin {
        push("plugin", vec![KdlValue::String(plugin.clone())]);
    }
    for (name, list) in [
        ("allowed_hosts", &web.allowed_hosts),
        ("allowed_origins", &web.allowed_origins),
        ("tailscale_users", &web.tailscale_users),
    ] {
        if !list.is_empty() {
            push(name, list.iter().cloned().map(KdlValue::String).collect());
        }
    }
    if let Some(days) = web.session_days {
        push("session_days", vec![KdlValue::Base10(i64::from(days))]);
    }
    if let Some(port) = web.https_port_hint {
        push("https_port_hint", vec![KdlValue::Base10(i64::from(port))]);
    }
    let mut node = KdlNode::new("web");
    node.set_children(children);
    Some(node)
}
