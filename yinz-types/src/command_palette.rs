//! Display-only Deno palette protocol. Callbacks never cross this boundary.
use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::data::PaneId;

pub const MAX_DISPLAY_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub registration: String,
    pub opening: u64,
    pub view: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub client_id: u32,
    pub pane_id: PaneId,
    pub tab_id: u64,
    pub current_project: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub handle: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Field {
    Text {
        id: String,
        label: String,
        #[serde(default, rename = "initialValue")]
        initial_value: String,
    },
    Directory {
        id: String,
        label: String,
        #[serde(default, rename = "initialValue")]
        initial_value: String,
    },
    Choice {
        id: String,
        label: String,
        options: Vec<String>,
        #[serde(default, rename = "initialValue")]
        initial_value: Option<String>,
    },
}

impl Field {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. } | Self::Directory { id, .. } | Self::Choice { id, .. } => id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum View {
    Actions {
        title: String,
        actions: Vec<Action>,
        source: Option<String>,
        #[serde(default, rename = "initialQuery")]
        initial_query: String,
    },
    Form {
        title: String,
        fields: Vec<Field>,
        #[serde(rename = "submitLabel")]
        submit_label: String,
        handle: String,
        #[serde(default)]
        actions: Vec<Action>,
        #[serde(default)]
        confirmation: bool,
    },
    Text {
        title: String,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Reply {
    Fields {
        values: BTreeMap<String, String>,
    },
    Actions {
        actions: Vec<Action>,
    },
    View {
        view: View,
    },
    Complete {
        notification: String,
        view: Option<View>,
    },
    Close,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Command {
    Suspend {
        scope: Scope,
        suspended: bool,
    },
    Register {
        id: String,
        title: String,
        registration: String,
        #[serde(default, rename = "staticActions")]
        static_actions: bool,
    },
    Unregister {
        registration: String,
    },
    Reply {
        scope: Scope,
        result: Result<Reply, String>,
    },
    Close {
        scope: Scope,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Message {
    Discover {
        scope: Scope,
        context: Context,
        query: String,
        source: Option<String>,
    },
    Execute {
        scope: Scope,
        context: Context,
        handle: String,
        values: Option<BTreeMap<String, String>>,
    },
    Cancel {
        scope: Scope,
    },
    Release {
        handles: Vec<String>,
    },
    Dismiss {
        opening: u64,
    },
    Disconnect,
}

pub fn label(value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err("Palette labels and IDs must be nonempty text without controls".into())
    } else {
        Ok(())
    }
}

pub fn plain_text(value: &str) -> Result<(), String> {
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        Err("Palette text must not contain terminal controls".into())
    } else {
        Ok(())
    }
}

pub fn validate_actions(actions: &[Action]) -> Result<(), String> {
    if actions.len() > 100 {
        return Err("Palette replies are limited to 100 actions".into());
    }
    let mut ids = HashSet::new();
    for action in actions {
        label(&action.id)?;
        label(&action.label)?;
        label(&action.handle)?;
        plain_text(&action.description)?;
        for alias in &action.aliases {
            label(alias)?;
        }
        if !ids.insert(&action.id) {
            return Err("Palette action IDs must be unique".into());
        }
    }
    Ok(())
}

impl Reply {
    pub fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_DISPLAY_BYTES {
            return Err("Palette display data exceeds 256 KiB".into());
        }
        match self {
            Self::Fields { values } => {
                for (id, value) in values {
                    label(id)?;
                    plain_text(value)?;
                }
                Ok(())
            },
            Self::Actions { actions } => validate_actions(actions),
            Self::Close => Ok(()),
            Self::Complete { notification, view } => {
                label(notification)?;
                if let Some(view) = view {
                    Self::View { view: view.clone() }.validate()?;
                }
                Ok(())
            },
            Self::View { view } => match view {
                View::Actions {
                    title,
                    actions,
                    source,
                    initial_query,
                } => {
                    plain_text(initial_query)?;
                    label(title)?;
                    if let Some(source) = source {
                        label(source)?;
                    }
                    validate_actions(actions)
                },
                View::Text { title, text } => {
                    label(title)?;
                    plain_text(text)
                },
                View::Form {
                    title,
                    fields,
                    submit_label,
                    handle,
                    actions,
                    ..
                } => {
                    label(title)?;
                    label(submit_label)?;
                    label(handle)?;
                    validate_actions(actions)?;
                    if fields.len() > 16 {
                        return Err("Palette forms are limited to 16 fields".into());
                    }
                    let mut ids = HashSet::new();
                    for field in fields {
                        label(field.id())?;
                        if !ids.insert(field.id()) {
                            return Err("Palette field IDs must be unique".into());
                        }
                        match field {
                            Field::Directory {
                                label: name,
                                initial_value,
                                ..
                            }
                            | Field::Text {
                                label: name,
                                initial_value,
                                ..
                            } => {
                                label(name)?;
                                plain_text(initial_value)?;
                            },
                            Field::Choice {
                                label: name,
                                options,
                                initial_value,
                                ..
                            } => {
                                label(name)?;
                                if options.is_empty()
                                    || initial_value.as_ref().is_some_and(|v| !options.contains(v))
                                {
                                    return Err(
                                        "Palette choices require options and a valid default"
                                            .into(),
                                    );
                                }
                                for option in options {
                                    label(option)?;
                                }
                            },
                        }
                    }
                    Ok(())
                },
            },
        }
    }

    pub fn handles(&self) -> Vec<String> {
        match self {
            Self::Complete {
                view: Some(view), ..
            } => Self::View { view: view.clone() }.handles(),
            Self::Actions { actions } => actions.iter().map(|a| a.handle.clone()).collect(),
            Self::View {
                view: View::Actions {
                    actions, source, ..
                },
            } => actions
                .iter()
                .map(|a| a.handle.clone())
                .chain(source.iter().cloned())
                .collect(),
            Self::View {
                view: View::Form {
                    handle, actions, ..
                },
            } => std::iter::once(handle.clone())
                .chain(actions.iter().map(|a| a.handle.clone()))
                .collect(),
            _ => vec![],
        }
    }
}

#[cfg(test)]
#[path = "unit/command_palette.rs"]
mod tests;
