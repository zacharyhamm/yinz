use super::runtime_parity::{default_reply, Pair, Responder};
use prost::Message;
use std::{collections::BTreeMap, sync::Arc};
use yinz_plugin_api::plugin_command::*;
use yinz_types::data::{
    Event, LayoutInfo, LayoutMetadata, PaneMetadata, PluginCommand, TabMetadata,
};

fn metadata() -> LayoutMetadata {
    LayoutMetadata {
        tabs: vec![TabMetadata {
            name: Some("work".into()),
            panes: vec![
                PaneMetadata {
                    name: Some("editor".into()),
                    is_plugin: false,
                    is_builtin_plugin: false,
                },
                PaneMetadata {
                    name: None,
                    is_plugin: false,
                    is_builtin_plugin: false,
                },
            ],
        }],
        ..Default::default()
    }
}
fn replies(command: &PluginCommand) -> Option<Vec<u8>> {
    Some(match command {
        PluginCommand::NewTabsWithLayoutInfo(..) => {
            ProtobufNewTabsResponse { tab_ids: vec![7] }.encode_to_vec()
        },
        PluginCommand::DeleteLayout { .. } => ProtobufDeleteLayoutResponse {
            result: Some(delete_layout_response::Result::Success(true)),
        }
        .encode_to_vec(),
        PluginCommand::RenameLayout { .. } => ProtobufRenameLayoutResponse {
            result: Some(rename_layout_response::Result::Success(true)),
        }
        .encode_to_vec(),
        PluginCommand::SaveLayout { .. } => ProtobufSaveLayoutResponse {
            result: Some(save_layout_response::Result::Success(true)),
        }
        .encode_to_vec(),
        PluginCommand::EditLayout { .. } => ProtobufEditLayoutResponse {
            result: Some(edit_layout_response::Result::Success(true)),
        }
        .encode_to_vec(),
        PluginCommand::DumpSessionLayout { .. } => ProtobufDumpSessionLayoutResponse {
            result: Some(dump_session_layout_response::Result::LayoutContent(
                "layout { pane; }".into(),
            )),
            metadata: Some(metadata().try_into().unwrap()),
        }
        .encode_to_vec(),
        PluginCommand::ParseLayout(..) => ProtobufParseLayoutResponse {
            result: Some(parse_layout_response::Result::Metadata(
                metadata().try_into().unwrap(),
            )),
        }
        .encode_to_vec(),
        PluginCommand::GenerateRandomName => ProtobufGenerateRandomNameResponse {
            name: "generated-name".into(),
        }
        .encode_to_vec(),
        PluginCommand::GetFocusedPaneInfo => ProtobufGetFocusedPaneInfoResponse {
            result: Some(get_focused_pane_info_response::Result::FocusedPaneInfo(
                FocusedPaneInfo {
                    focused_tab_id: 7,
                    focused_pane_id: Some(ProtobufPaneId {
                        pane_type: 0,
                        id: 3,
                    }),
                },
            )),
        }
        .encode_to_vec(),
        _ => return default_reply(command),
    })
}
fn pair() -> Pair {
    Pair::with_host("layout-manager", BTreeMap::new(), Arc::new(replies), None)
}
fn layouts(pair: &mut Pair) {
    pair.event(Event::AvailableLayoutInfo(
        vec![
            LayoutInfo::BuiltIn("compact".into()),
            LayoutInfo::BuiltIn("default".into()),
            LayoutInfo::File("alpha.kdl".into(), metadata()),
            LayoutInfo::File("雪-work.kdl".into(), metadata()),
        ],
        vec![],
    ));
}
#[test]
fn layout_manager_actual_runtimes_match_search_management_and_apply() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "layout_manager_actual_runtimes_match_search_management_and_apply",
    );
    let mut pair = pair();
    pair.render(24, 100);
    layouts(&mut pair);
    for (rows, cols) in [(24, 100), (12, 60), (8, 30), (30, 140)] {
        pair.render(rows, cols);
    }
    for key in [
        "Down", "Up", "w", "Tab", "Esc", "Esc", "?", "t", "t", "t", "t", "a", "Down",
    ] {
        pair.key(key);
        pair.render(24, 100);
    }
    pair.key("Alt w");
}
#[test]
fn layout_manager_actual_runtimes_match_rename_new_and_import_forms() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "layout_manager_actual_runtimes_match_rename_new_and_import_forms",
    );
    let mut pair = pair();
    layouts(&mut pair);
    pair.render(24, 100);
    pair.key("Esc");
    pair.key("r");
    pair.render(24, 100);
    pair.event(Event::PastedText("renamed".into()));
    pair.render(24, 100);
    for key in ["Enter", "Tab", "Enter"] {
        pair.key(key);
        pair.render(24, 100);
    }
    pair.key("Esc");
    pair.key("n");
    pair.render(24, 100);
    pair.event(Event::PastedText("snapshot".into()));
    pair.render(24, 100);
    for key in ["Enter", "Tab", "Enter", "Down", "Enter", "Tab", "Enter"] {
        pair.key(key);
        pair.render(24, 100);
    }
    pair.key("Esc");
    pair.key("i");
    pair.render(24, 100);
    pair.event(Event::PastedText("layout { pane; }".into()));
    pair.render(24, 100);
    pair.key("r");
    pair.event(Event::Key("x".parse().unwrap()));
    pair.render(24, 100);
    pair.key("Enter");
    pair.key("Enter");
    pair.render(24, 100);
}

#[test]
fn layout_manager_actual_runtimes_match_refused_mutations_and_layout_diagnostics() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "layout_manager_actual_runtimes_match_refused_mutations_and_layout_diagnostics",
    );
    use yinz_types::data::{LayoutParsingError, LayoutWithError};
    let host: Responder = Arc::new(|command| {
        Some(match command {
            PluginCommand::DeleteLayout { .. } => ProtobufDeleteLayoutResponse {
                result: Some(delete_layout_response::Result::Error(
                    "read-only file system".into(),
                )),
            }
            .encode_to_vec(),
            PluginCommand::SaveLayout { .. } => ProtobufSaveLayoutResponse {
                result: Some(save_layout_response::Result::Error("disk is full".into())),
            }
            .encode_to_vec(),
            PluginCommand::ParseLayout(source) => {
                let error = yinz_config::kdl::layout::from_kdl(
                    source,
                    Some("parse_layout_api".into()),
                    None,
                    None,
                )
                .unwrap_err();
                let error = match error {
                    yinz_config::config::ConfigError::KdlError(error) => {
                        LayoutParsingError::KdlError {
                            kdl_error: Box::new(error.into()),
                            file_name: "parse_layout_api".into(),
                            source_code: source.clone(),
                        }
                    },
                    _ => LayoutParsingError::SyntaxError,
                };
                ProtobufParseLayoutResponse {
                    result: Some(parse_layout_response::Result::Error(
                        error.try_into().unwrap(),
                    )),
                }
                .encode_to_vec()
            },
            _ => return replies(command),
        })
    });
    let mut pair = Pair::with_host("layout-manager", BTreeMap::new(), host, None);
    layouts(&mut pair);
    pair.render(24, 100);
    pair.key("Esc");
    pair.key("Delete");
    pair.viewports();
    pair.key("x");
    pair.render(24, 100);
    pair.key("i");
    pair.key("Enter");
    pair.viewports();
    pair.key("x");
    pair.event(Event::PastedText("layout { pane size=oops; }".into()));
    pair.viewports();
    pair.key("Enter");
    pair.viewports();
    pair.key("x");
    pair.key("Esc");
    pair.event(Event::AvailableLayoutInfo(
        vec![],
        vec![LayoutWithError {
            layout_name: "broken.kdl".into(),
            error: LayoutParsingError::SyntaxError,
        }],
    ));
    pair.render(24, 100);
    pair.key("Esc");
    pair.key("m");
    pair.viewports();
    pair.key("x");
    pair.render(24, 100);
}

#[test]
fn layout_manager_actual_runtimes_match_long_lists_multitab_details_and_mouse_save() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "layout_manager_actual_runtimes_match_long_lists_multitab_details_and_mouse_save",
    );
    use yinz_types::data::Mouse;
    let mut pair = pair();
    let mut multi = metadata();
    multi.tabs.push(TabMetadata {
        name: None,
        panes: vec![
            PaneMetadata {
                name: Some("雪-wide-editor".into()),
                is_plugin: false,
                is_builtin_plugin: false,
            },
            PaneMetadata {
                name: Some("ignored builtin".into()),
                is_plugin: true,
                is_builtin_plugin: true,
            },
        ],
    });
    pair.event(Event::AvailableLayoutInfo(
        (0..35)
            .map(|i| LayoutInfo::File(format!("{i:02}-layout-with-a-long-name.kdl"), multi.clone()))
            .collect(),
        vec![],
    ));
    pair.viewports();
    for _ in 0..18 {
        pair.key("Down");
    }
    pair.viewports();
    pair.key("Esc");
    pair.key("r");
    pair.viewports();
    pair.render(24, 100);
    pair.event(Event::PastedText("雪".into()));
    pair.render(24, 100);
    pair.event(Event::Mouse(Mouse::LeftClick(12, 4)));
    let commands = pair.event(Event::Mouse(Mouse::Release(12, 4)));
    assert!(
        commands
            .iter()
            .any(|command| command.get("RenameLayout").is_some()),
        "save click must activate the actual button: {commands:?}"
    );
    pair.render(24, 100);
}
