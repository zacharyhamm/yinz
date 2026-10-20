//! Offline checks run only in the supervised compilation process.

use std::path::Path;
use yinz_runtime::errors::prelude::*;
use yinz_types::authoring::{OutlineEntry, OutlineKind, PluginCheckMode, PluginCheckReport};
use yinz_types::data::PermissionType;
use yinz_types::plugin_data::{validate_namespace, RESERVED_ATTR_NAMES, SHARED_NAMESPACE_PREFIX};

pub(crate) fn check(
    path: &Path,
    source: &str,
    mode: PluginCheckMode,
    root: Option<&Path>,
) -> PluginCheckReport {
    let checked = (|| -> Result<PluginCheckReport> {
        if matches!(mode, PluginCheckMode::Semantic | PluginCheckMode::Prepare) {
            let mut runtime = yinz_steel::Runtime::new(|_| {
                bail!("host commands are unavailable while checking a plugin")
            })?;
            let mut root = super::source_root(path, root)?;
            runtime
                .check_with_source_root(source, &mut root)
                .with_context(|| format!("checking {}", path.display()))?;
        }
        let analysis = yinz_steel::analysis::analyze(source)
            .with_context(|| format!("checking {}", path.display()))?;
        let mut report = describe(analysis);
        if matches!(mode, PluginCheckMode::Format | PluginCheckMode::Prepare) {
            report.formatted_source = Some(yinz_steel::format::format(source)?);
        }
        Ok(report)
    })();
    match checked {
        Ok(report) => report,
        Err(error) => PluginCheckReport {
            diagnostics: vec![format!("{error:#}")],
            ..Default::default()
        },
    }
}

fn describe(analysis: yinz_steel::analysis::Analysis) -> PluginCheckReport {
    let mut report = PluginCheckReport::default();
    for (line, text) in analysis.definitions {
        report.outline.push(OutlineEntry {
            line,
            kind: if text.starts_with('(') {
                OutlineKind::Function
            } else {
                OutlineKind::Define
            },
            text,
        });
    }
    for (line, text) in analysis.imports {
        report.outline.push(OutlineEntry {
            line,
            kind: OutlineKind::Require,
            text,
        });
    }
    for command in &analysis.commands {
        if command.name != "RequestPluginPermissions" {
            continue;
        }
        report.outline.push(OutlineEntry {
            line: command.line,
            kind: OutlineKind::Permission,
            text: command
                .argument
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "computed permissions".into()),
        });
        let Some(permissions) = command
            .argument
            .as_ref()
            .and_then(serde_json::Value::as_array)
        else {
            report.warnings.push(format!("line {}: computed permissions cannot be inferred; use a literal list of permission strings", command.line));
            continue;
        };
        for permission in permissions {
            if let Ok(permission) = serde_json::from_value::<PermissionType>(permission.clone()) {
                if !report.permissions.contains(&permission) {
                    report.permissions.push(permission);
                }
            } else {
                let suggestion = permission
                    .as_str()
                    .and_then(yinz_plugin_api::authoring::nearest_permission_type)
                    .map(|value| format!("; did you mean {value:?}?"))
                    .unwrap_or_default();
                report.warnings.push(format!(
                    "line {}: {permission} is not a permission{suggestion}",
                    command.line
                ));
            }
        }
    }
    let data_commands: Vec<_> = analysis
        .commands
        .iter()
        .filter(|command| {
            matches!(
                command.name.as_str(),
                "DataDefineEntity"
                    | "DataApply"
                    | "DataFetch"
                    | "DataCount"
                    | "DataWatch"
                    | "DataUnwatch"
                    | "DataCatalog"
            )
        })
        .collect();
    if !data_commands.is_empty() && !report.permissions.contains(&PermissionType::PluginData) {
        report
            .warnings
            .push("the data store requires a request for PluginData permission".into());
    }
    if let Some(command) = data_commands.iter().find(|command| command.during_load) {
        report.warnings.push(format!(
            "line {}: load must wait for PermissionRequestResult before accessing the data store",
            command.line
        ));
    }
    for command in data_commands {
        if let Some(argument) = &command.argument {
            namespaces(argument, command.line, &mut report);
            if command.name == "DataDefineEntity"
                && let Some(attributes) = argument
                    .pointer("/def/attrs")
                    .and_then(serde_json::Value::as_array)
            {
                for attribute in attributes {
                    if let Some(name) = attribute.get("name").and_then(serde_json::Value::as_str)
                        && RESERVED_ATTR_NAMES.contains(&name)
                    {
                        report.warnings.push(format!("line {}: {name:?} is a reserved attribute already present on every row", command.line));
                    }
                }
            }
        }
    }
    report.outline.sort_by_key(|entry| entry.line);
    report
}

fn namespaces(value: &serde_json::Value, line: usize, report: &mut PluginCheckReport) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(namespace) = object.get("namespace").and_then(serde_json::Value::as_str) {
                if let Err(error) = validate_namespace(Some(namespace)) {
                    report.warnings.push(format!("line {line}: {error}"));
                }
                if namespace.starts_with(SHARED_NAMESPACE_PREFIX)
                    && !report
                        .permissions
                        .contains(&PermissionType::SharedPluginData)
                {
                    report.warnings.push(format!("line {line}: a shared: namespace also requires SharedPluginData permission"));
                }
            }
            for value in object.values() {
                namespaces(value, line, report);
            }
        },
        serde_json::Value::Array(values) => {
            for value in values {
                namespaces(value, line, report);
            }
        },
        _ => {},
    }
}

#[cfg(test)]
#[path = "unit/authoring.rs"]
mod tests;
