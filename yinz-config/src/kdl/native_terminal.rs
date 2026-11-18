use super::*;
use crate::kdl_parsing_error;
use yinz_types::input::native_terminal::TerminalConfig;

pub(crate) fn from_kdl(block: &KdlNode) -> Result<TerminalConfig, ConfigError> {
    if !block.entries().is_empty() {
        return Err(kdl_parsing_error!(
            "terminal takes child settings".to_owned(),
            block
        ));
    }
    let mut object = serde_json::Map::new();
    for node in block
        .children()
        .into_iter()
        .flat_map(|children| children.nodes())
    {
        if node.name().value() == "font_features" {
            let features = font_features_from_kdl(node)?;
            if object
                .insert("font_features".to_owned(), features.into())
                .is_some()
            {
                return Err(kdl_parsing_error!(
                    "duplicate terminal setting font_features".to_owned(),
                    node
                ));
            }
            continue;
        }
        let name = node.name().value();
        let plain_positional = node.children().is_none()
            && node.entries().len() == 1
            && node.entries()[0].name().is_none();
        if matches!(name, "initial_columns" | "initial_rows")
            && (!plain_positional
                || node.ty().is_some()
                || node.entries()[0].ty().is_some()
                || !node.entries()[0]
                    .value()
                    .as_i64()
                    .is_some_and(|value| (2..=1000).contains(&value)))
        {
            return Err(kdl_parsing_error!(
                format!("terminal {name} must be a plain integer between 2 and 1000 cells"),
                node
            ));
        }
        if !plain_positional {
            return Err(kdl_parsing_error!(
                format!("terminal {} takes one plain value", node.name().value()),
                node
            ));
        }
        let raw_name = node.name().value();
        let name = match raw_name {
            "font_family" => "font",
            "font_ligatures" => "ligatures",
            _ => raw_name,
        };
        let value = node.entries()[0].value();
        if name == "ligatures"
            && (value.as_bool().is_none()
                || node.ty().is_some()
                || node.entries()[0].ty().is_some())
        {
            return Err(kdl_parsing_error!(
                format!("terminal {raw_name} (ligatures/font_ligatures) must be a plain boolean true or false"),
                node
            ));
        }
        if matches!(
            name,
            "baseline_offset" | "underline_offset" | "underline_thickness"
        ) && (node.ty().is_some()
            || node.entries()[0].ty().is_some()
            || !value
                .as_i64()
                .is_some_and(|value| (-64..=64).contains(&value)))
        {
            return Err(kdl_parsing_error!(
                format!(
                    "terminal {name} must be a plain integer between -64 and 64 logical pixels"
                ),
                node
            ));
        }
        if matches!(name, "cell_width" | "line_height") {
            let numeric = value
                .as_i64()
                .map(|value| value as f64)
                .or_else(|| value.as_f64());
            if node.ty().is_some() || node.entries()[0].ty().is_some() || numeric.is_none() {
                return Err(kdl_parsing_error!(
                    format!("terminal {name} must be a plain numeric multiplier between 0.5 and 3"),
                    node
                ));
            }
            if numeric.is_some_and(|value| !value.is_finite() || !(0.5..=3.0).contains(&value)) {
                return Err(kdl_parsing_error!(
                    format!("terminal {name} must be a finite multiplier between 0.5 and 3"),
                    node
                ));
            }
        }
        let json = if let Some(value) = value.as_string() {
            serde_json::Value::from(value)
        } else if let Some(value) = value.as_bool() {
            serde_json::Value::from(value)
        } else if let Some(value) = value.as_i64() {
            serde_json::Value::from(value)
        } else if let Some(value) = value.as_f64() {
            serde_json::Value::from(value)
        } else {
            return Err(kdl_parsing_error!(
                "invalid terminal value".to_owned(),
                node
            ));
        };
        if let Some(previous) = object.insert(name.to_owned(), json.clone()) {
            return Err(kdl_parsing_error!(
                if name == "font" {
                    format!("duplicate terminal font/font_family setting: earlier value {previous}, conflicting {raw_name} {json}; use only one spelling per block")
                } else if name == "ligatures" {
                    format!("duplicate terminal ligatures/font_ligatures setting: earlier value {previous}, conflicting {raw_name} {json}; use only one spelling per block")
                } else {
                    format!("duplicate terminal setting {name}")
                },
                node
            ));
        }
    }
    let config: TerminalConfig = serde_json::from_value(object.into())
        .map_err(|error| kdl_parsing_error!(error.to_string(), block))?;
    for (name, value, min, max) in [
        ("font_size", config.font_size, 3.0, 384.0),
        ("padding", config.padding, 0.0, 256.0),
        ("opacity", config.opacity, 0.0, 1.0),
    ] {
        if value.is_some_and(|value| !value.is_finite() || value < min || value > max) {
            return Err(kdl_parsing_error!(
                format!("terminal {name} must be between {min} and {max}"),
                block
            ));
        }
    }
    if config
        .font_weight
        .is_some_and(|weight| !(100..=900).contains(&weight))
    {
        return Err(kdl_parsing_error!(
            "font_weight must be between 100 and 900".to_owned(),
            block
        ));
    }
    Ok(config)
}
/// One `font_features` node with zero or more plain string arguments; a bare node
/// is an explicitly empty list. Each entry is validated with its own span.
fn font_features_from_kdl(node: &KdlNode) -> Result<Vec<String>, ConfigError> {
    if node.children().is_some() || node.ty().is_some() {
        return Err(kdl_parsing_error!(
            "terminal font_features takes plain string arguments only, without a type annotation or children".to_owned(),
            node
        ));
    }
    let mut features = Vec::with_capacity(node.entries().len());
    for entry in node.entries() {
        if entry.name().is_some() || entry.ty().is_some() {
            return Err(kdl_parsing_error!(
                "terminal font_features: a font feature must be a plain string argument, without a name or type annotation".to_owned(),
                entry
            ));
        }
        let Some(text) = entry.value().as_string() else {
            return Err(kdl_parsing_error!(
                "terminal font_features: a font feature must be given as a string".to_owned(),
                entry
            ));
        };
        if yinz_types::input::native_terminal::parse_font_feature(text).is_none() {
            return Err(kdl_parsing_error!(
                format!("terminal font_features: {text:?} is not a font feature; use a four-character OpenType tag such as \"ss01\", optionally prefixed with + to turn it on or - to turn it off"),
                entry
            ));
        }
        features.push(text.to_owned());
    }
    Ok(features)
}

pub(crate) fn merge(
    base: &TerminalConfig,
    overlay: TerminalConfig,
) -> Result<TerminalConfig, ConfigError> {
    if let (Some(previous), Some(winning)) = (&base.font_family, &overlay.font_family)
        && previous != winning
        && !(previous.trim().is_empty() && winning.trim().is_empty())
    {
        log::warn!("terminal font/font_family override: {previous:?} -> {winning:?}; later configuration layer wins");
    }
    let mut base = serde_json::to_value(base)
        .map_err(|error| ConfigError::new_kdl_error(error.to_string(), 0, 0))?;
    let overlay = serde_json::to_value(overlay)
        .map_err(|error| ConfigError::new_kdl_error(error.to_string(), 0, 0))?;
    if let (Some(base), Some(overlay)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, value) in overlay {
            if !value.is_null() {
                base.insert(key.clone(), value.clone());
            }
        }
    }
    serde_json::from_value(base)
        .map_err(|error| ConfigError::new_kdl_error(error.to_string(), 0, 0))
}
pub(crate) fn to_kdl(config: &TerminalConfig) -> Option<KdlNode> {
    let object = serde_json::to_value(config).ok()?;
    let mut children = KdlDocument::new();
    for (name, value) in object.as_object()? {
        if let serde_json::Value::Array(features) = value {
            // A bare node keeps an explicitly empty list distinct from omission.
            let mut node = KdlNode::new(name.as_str());
            for feature in features {
                node.push(KdlValue::String(feature.as_str()?.to_owned()));
            }
            children.nodes_mut().push(node);
            continue;
        }
        let value = match value {
            serde_json::Value::String(value) => KdlValue::String(value.clone()),
            serde_json::Value::Bool(value) => KdlValue::Bool(*value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    KdlValue::Base10(value)
                } else {
                    KdlValue::Base10Float(value.as_f64()?)
                }
            },
            _ => continue,
        };
        let mut node = KdlNode::new(name.as_str());
        node.push(value);
        children.nodes_mut().push(node);
    }
    if children.nodes().is_empty() {
        return None;
    }
    let mut node = KdlNode::new("terminal");
    node.set_children(children);
    Some(node)
}
