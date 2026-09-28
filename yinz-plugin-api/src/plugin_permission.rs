pub use super::generated_api::api::plugin_permission::PermissionType as ProtobufPermissionType;
use yinz_types::data::PermissionType;

use std::convert::TryFrom;

impl TryFrom<ProtobufPermissionType> for PermissionType {
    type Error = &'static str;
    fn try_from(protobuf_permission: ProtobufPermissionType) -> Result<Self, &'static str> {
        match protobuf_permission {
            ProtobufPermissionType::ReadApplicationState => {
                Ok(PermissionType::ReadApplicationState)
            },
            ProtobufPermissionType::ChangeApplicationState => {
                Ok(PermissionType::ChangeApplicationState)
            },
            ProtobufPermissionType::OpenFiles => Ok(PermissionType::OpenFiles),
            ProtobufPermissionType::RunCommands => Ok(PermissionType::RunCommands),
            ProtobufPermissionType::OpenTerminalsOrPlugins => {
                Ok(PermissionType::OpenTerminalsOrPlugins)
            },
            ProtobufPermissionType::WriteToStdin => Ok(PermissionType::WriteToStdin),
            ProtobufPermissionType::WebAccess => Ok(PermissionType::WebAccess),
            ProtobufPermissionType::ReadCliPipes => Ok(PermissionType::ReadCliPipes),
            ProtobufPermissionType::MessageAndLaunchOtherPlugins => {
                Ok(PermissionType::MessageAndLaunchOtherPlugins)
            },
            ProtobufPermissionType::Reconfigure => Ok(PermissionType::Reconfigure),
            ProtobufPermissionType::FullHdAccess => Ok(PermissionType::FullHdAccess),
            ProtobufPermissionType::InterceptInput => Ok(PermissionType::InterceptInput),
            ProtobufPermissionType::ReadPaneContents => Ok(PermissionType::ReadPaneContents),
            ProtobufPermissionType::RunActionsAsUser => Ok(PermissionType::RunActionsAsUser),
            ProtobufPermissionType::WriteToClipboard => Ok(PermissionType::WriteToClipboard),
            ProtobufPermissionType::ReadSessionEnvironmentVariables => {
                Ok(PermissionType::ReadSessionEnvironmentVariables)
            },
            ProtobufPermissionType::PluginData => Ok(PermissionType::PluginData),
            ProtobufPermissionType::SharedPluginData => Ok(PermissionType::SharedPluginData),
            ProtobufPermissionType::ReadFiles => Ok(PermissionType::ReadFiles),
            ProtobufPermissionType::WriteFiles => Ok(PermissionType::WriteFiles),
            ProtobufPermissionType::AuthorPlugins => Ok(PermissionType::AuthorPlugins),
        }
    }
}

impl TryFrom<PermissionType> for ProtobufPermissionType {
    type Error = &'static str;
    fn try_from(permission: PermissionType) -> Result<Self, &'static str> {
        match permission {
            PermissionType::ReadApplicationState => {
                Ok(ProtobufPermissionType::ReadApplicationState)
            },
            PermissionType::ChangeApplicationState => {
                Ok(ProtobufPermissionType::ChangeApplicationState)
            },
            PermissionType::OpenFiles => Ok(ProtobufPermissionType::OpenFiles),
            PermissionType::RunCommands => Ok(ProtobufPermissionType::RunCommands),
            PermissionType::OpenTerminalsOrPlugins => {
                Ok(ProtobufPermissionType::OpenTerminalsOrPlugins)
            },
            PermissionType::WriteToStdin => Ok(ProtobufPermissionType::WriteToStdin),
            PermissionType::WebAccess => Ok(ProtobufPermissionType::WebAccess),
            PermissionType::ReadCliPipes => Ok(ProtobufPermissionType::ReadCliPipes),
            PermissionType::MessageAndLaunchOtherPlugins => {
                Ok(ProtobufPermissionType::MessageAndLaunchOtherPlugins)
            },
            PermissionType::Reconfigure => Ok(ProtobufPermissionType::Reconfigure),
            PermissionType::FullHdAccess => Ok(ProtobufPermissionType::FullHdAccess),
            PermissionType::InterceptInput => Ok(ProtobufPermissionType::InterceptInput),
            PermissionType::ReadPaneContents => Ok(ProtobufPermissionType::ReadPaneContents),
            PermissionType::RunActionsAsUser => Ok(ProtobufPermissionType::RunActionsAsUser),
            PermissionType::WriteToClipboard => Ok(ProtobufPermissionType::WriteToClipboard),
            PermissionType::ReadSessionEnvironmentVariables => {
                Ok(ProtobufPermissionType::ReadSessionEnvironmentVariables)
            },
            PermissionType::PluginData => Ok(ProtobufPermissionType::PluginData),
            PermissionType::SharedPluginData => Ok(ProtobufPermissionType::SharedPluginData),
            PermissionType::ReadFiles => Ok(ProtobufPermissionType::ReadFiles),
            PermissionType::WriteFiles => Ok(ProtobufPermissionType::WriteFiles),
            PermissionType::AuthorPlugins => Ok(ProtobufPermissionType::AuthorPlugins),
        }
    }
}
