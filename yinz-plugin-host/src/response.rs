//! Guest-visible replies to synchronous Steel host commands.
use serde::Serialize;
use yinz_runtime::errors::prelude::*;
use yinz_types::data::{CommandType, LayoutParsingError, PermissionType};

pub(crate) const DATA_CONFLICT_MARKER: &str = "data conflict: ";

/// A decoded host reply. File contents stay binary, including invalid UTF-8.
#[derive(Debug, PartialEq)]
pub enum DecodedResponse {
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}

/// Guest errors are the operation's answer. Only a failure to dispatch a command
/// carries the additional Steel host-command context.
#[derive(Debug)]
pub(crate) struct GuestError(String);

impl std::fmt::Display for GuestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for GuestError {}

pub(crate) fn guest_error(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(GuestError(format!("{error:#}")))
}

/// Preserve the source location, parser message and hint in the raised guest error.
pub(crate) fn layout_parsing_error(error: LayoutParsingError) -> anyhow::Error {
    match error {
        LayoutParsingError::KdlError {
            kdl_error,
            file_name,
            ..
        } => match kdl_error.help_message {
            Some(help) => anyhow!("{}: {} ({})", file_name, kdl_error.error_message, help),
            None => anyhow!("{}: {}", file_name, kdl_error.error_message),
        },
        LayoutParsingError::SyntaxError => anyhow!("the layout could not be parsed"),
    }
}

pub(crate) fn value<T: Serialize>(from: T) -> Result<DecodedResponse> {
    serde_json::to_value(from)
        .map(DecodedResponse::Json)
        .context("yinz's answer could not be serialized")
}

pub(crate) fn git_probe(probe: yinz_types::data::PaneGitProbe) -> Result<DecodedResponse> {
    Ok(DecodedResponse::Json(serde_json::json!({
        "host": probe.host,
        "cwd": probe.cwd,
        "exit_code": probe.exit_code,
        "metadata": yinz_steel::binary::encode(&probe.metadata)?,
        "config": yinz_steel::binary::encode(&probe.config)?,
    })))
}

/// Direct host callers receive command-specific denials. The Steel dispatcher reports
/// missing and pending permissions before invoking a handler.
pub(crate) fn denied(
    kind: CommandType,
    permission: Option<PermissionType>,
) -> Result<DecodedResponse> {
    use CommandType::*;
    match kind {
        InspectConfiguration
        | EditConfiguration
        | FsRead
        | FsList
        | FsMetadata
        | FsWrite
        | FsRemove
        | FsCreateDir
        | FsRename
        | DumpSessionLayout
        | GetPaneGitProbe
        | OpenCommandPaneInGitCheckoutNearPlugin
        | SaveLayout
        | DeleteLayout
        | RenameLayout
        | EditLayout
        | DumpLayout
        | ParseLayout
        | GetFocusedPaneInfo
        | SaveSession
        | HideFloatingPanes
        | ShowFloatingPanes
        | CheckPlugin
        | ProjectConfigRead
        | ProjectConfigWrite
        | SpawnAgent
        | ReadPluginLog => Err(anyhow!("yinz answered with nothing")),
        PaneTabs
        | CreateMultipane
        | UpdateMultipane
        | ReplaceMultipaneChild
        | RestartAgentPane
        | RegisterAgentProvider
        | GetAgentProviders
        | RegisterProjectProvider
        | GetProjectProviders
        | ValidateProjectMatch
        | PublishProjectLocations
        | ResolveProject => Err(anyhow!("Permission denied: {permission:?}")),
        GetPluginIds => value(
            serde_json::json!({"plugin_id":0,"yinz_pid":0,"initial_cwd":"","client_id":0,"hostname":"","host_id":"","max_session_name_bytes":null}),
        ),
        GetYinzVersion | GenerateRandomName | GetLayoutDir | GetUserPluginDir | GetLocalTime => {
            value("")
        },
        NewTabsWithLayout | NewTabsWithLayoutInfo | ListNotifications => {
            value(serde_json::json!([]))
        },
        GetPaneScrollback => Err(anyhow!("PaneScrollbackResponse missing response field")),
        GetSessionEnvironmentVariables | GetPluginAliasConfiguration => {
            value(serde_json::json!({}))
        },
        GetPluginIdsForUrl | GetPluginIdsForUrlWithConfiguration | GetDisconnectTargets => {
            Err(anyhow!("Permission denied: ReadApplicationState"))
        },
        GetPluginInfo => Err(anyhow!(
            "decode plugin info: EOF while parsing a value at line 1 column 0"
        )),
        Pick => Err(anyhow!(
            "decode picker response: EOF while parsing a value at line 1 column 0"
        )),
        OpenCommandPaneInNewTab | OpenPluginPaneInNewTab | OpenEditorPaneInNewTab => {
            value(yinz_types::data::OpenPaneInNewTabResponse::default())
        },
        GetSessionList => Err(anyhow!("Empty GetSessionListResponse")),
        GetSessionInventory | SwitchSessionDestination => Err(anyhow!(
            "decode session destination response: EOF while parsing a value at line 1 column 0"
        )),
        KillSessionsAndReply | DeleteDeadSessionAndReply | DeleteAllDeadSessionsAndReply => {
            Err(anyhow!("Permission denied: ChangeApplicationState"))
        },
        WarpPaneToSession | WarpPaneFromSession | WarpTabToSession | WarpTabFromSession
        | AdoptOrphanedPane => Err(anyhow!("WarpPaneResponse without a result")),
        ListOrphanedPanes => Err(anyhow!("ListOrphanedPanesResponse without a result")),
        DataDefineEntity => Err(anyhow!("DataDefineResponse without a result")),
        DataApply => Err(anyhow!("DataApplyResponse without a result")),
        DataFetch => Err(anyhow!("DataFetchResponse without a result")),
        DataCount => Err(anyhow!("DataCountResponse without a result")),
        DataWatch => Err(anyhow!("DataWatchResponse without a result")),
        DataUnwatch => Err(anyhow!("DataUnwatchResponse without a result")),
        DataCatalog => Err(anyhow!("DataCatalogResponse without a result")),
        GetLocalUtcOffset => value(0),
        _ => value(()),
    }
}

#[cfg(test)]
impl DecodedResponse {
    pub(crate) fn expect_json(self) -> serde_json::Value {
        match self {
            Self::Json(value) => value,
            Self::Bytes(_) => panic!("expected a JSON reply"),
        }
    }
}

#[cfg(test)]
#[path = "unit/response.rs"]
mod tests;
