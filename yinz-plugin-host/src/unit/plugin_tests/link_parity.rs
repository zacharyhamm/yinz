use super::runtime_parity::{default_reply, Responder, SteelReference, WasmReference};
use prost::Message;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::Arc,
};
use yinz_plugin_api::plugin_command::*;
use yinz_types::{
    data::{Event, FileMetadata, PaneInfo, PaneManifest, PluginCommand},
    internal_ids::{TabPosition, TerminalId},
    pane_id::PaneId,
};

fn host(root: &Path) -> Responder {
    let root = root.to_owned();
    Arc::new(move |command| {
        Some(match command {
            PluginCommand::GetSessionEnvironmentVariables => {
                ProtobufGetSessionEnvironmentVariablesResponse {
                    env_vars: vec![
                        EnvVariable {
                            name: "HOME".into(),
                            value: "/work".into(),
                        },
                        EnvVariable {
                            name: "PROJECT".into(),
                            value: "/work".into(),
                        },
                    ],
                }
                .encode_to_vec()
            },
            PluginCommand::GetPaneCwd { pane_id } => ProtobufGetPaneCwdResponse {
                result: Some(if *pane_id == PaneId::Terminal(TerminalId::new(99)) {
                    get_pane_cwd_response::Result::Error("unavailable".into())
                } else {
                    get_pane_cwd_response::Result::Cwd("/work".into())
                }),
            }
            .encode_to_vec(),
            PluginCommand::FsList { path } => {
                let real = root.join(path.strip_prefix("/host").unwrap());
                ProtobufFsListResponse {
                    result: Some(match std::fs::read_dir(real) {
                        Ok(entries) => fs_list_response::Result::Entries(ProtobufFsEntryList {
                            entries: entries
                                .flatten()
                                .map(|entry| ProtobufFsEntry {
                                    path: path.join(entry.file_name()).to_str().unwrap().into(),
                                    metadata: Some(
                                        entry.metadata().ok().map(FileMetadata::from).into(),
                                    ),
                                })
                                .collect(),
                        }),
                        Err(error) => fs_list_response::Result::Error(error.to_string()),
                    }),
                }
                .encode_to_vec()
            },
            PluginCommand::FsMetadata {
                path,
                follow_symlinks,
            } => {
                assert!(
                    follow_symlinks,
                    "link must follow targets as the original std::fs::metadata did"
                );
                let real = root.join(path.strip_prefix("/host").unwrap());
                ProtobufFsMetadataResponse {
                    result: Some(match std::fs::metadata(real) {
                        Ok(metadata) => fs_metadata_response::Result::Metadata(
                            Some(FileMetadata::from(metadata)).into(),
                        ),
                        Err(_) => fs_metadata_response::Result::NotFound(true),
                    }),
                }
                .encode_to_vec()
            },
            PluginCommand::OpenFileFloating(..) => {
                ProtobufOpenFileFloatingResponse::default().encode_to_vec()
            },
            _ => return default_reply(command),
        })
    })
}
struct Pair {
    wasm: WasmReference,
    steel: SteelReference,
    root: tempfile::TempDir,
}
impl Pair {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("work/subdir")).unwrap();
        for name in [
            "alpha.rs",
            "café.txt",
            "a+b[0].rs",
            ".hidden",
            "$UNKNOWN",
            "~nobody",
        ] {
            std::fs::write(root.path().join("work").join(name), "file").unwrap();
        }
        let responder = host(root.path());
        let pair = Self {
            wasm: WasmReference::new("link", BTreeMap::new(), root.path(), responder.clone()),
            steel: SteelReference::new("link", BTreeMap::new(), root.path(), responder),
            root,
        };
        pair.commands();
        pair
    }
    fn commands(&self) -> Vec<Value> {
        let normalize = |commands: Vec<Value>| {
            commands
                .into_iter()
                .filter(|command| {
                    // Wasm reads these same fixture paths through WASI; Scheme uses the FS host bridge.
                    command.get("FsList").is_none() && command.get("FsMetadata").is_none()
                })
                .map(|mut command| {
                    if let Some(highlights) = command
                        .get_mut("SetPaneRegexHighlights")
                        .and_then(Value::as_array_mut)
                        .and_then(|args| args[1].as_array_mut())
                    {
                        // read_dir does not promise entry order. The general path regex stays first.
                        highlights[1..].sort_by_key(Value::to_string);
                    }
                    command
                })
                .collect::<Vec<_>>()
        };
        let old = normalize(self.wasm.take_commands());
        assert_eq!(normalize(self.steel.take_commands()), old);
        old
    }
    fn event(&mut self, event: Event) -> Vec<Value> {
        assert_eq!(
            self.steel.update(&event),
            self.wasm.update(&event),
            "{event:?}"
        );
        self.commands()
    }
    fn click(&mut self, text: &str, context: bool) -> Vec<Value> {
        self.event(Event::HighlightClicked {
            pane_id: PaneId::Terminal(TerminalId::new(7)),
            pattern: "irrelevant".into(),
            matched_string: text.into(),
            context: if context {
                BTreeMap::from([("cwd".into(), "/work".into())])
            } else {
                BTreeMap::new()
            },
        })
    }
}
fn panes(id: Option<u32>) -> Event {
    Event::PaneUpdate(PaneManifest {
        panes: HashMap::from([(
            TabPosition::new(0),
            id.map(|id| PaneInfo {
                id: PaneId::Terminal(TerminalId::new(id)),
                ..Default::default()
            })
            .into_iter()
            .collect(),
        )]),
    })
}

#[test]
fn link_actual_wasm_and_steel_match_discovery_cwd_changes_and_highlights() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "link_actual_wasm_and_steel_match_discovery_cwd_changes_and_highlights",
    );
    let mut pair = Pair::new();
    let initial = pair.event(panes(Some(7)));
    assert_eq!(initial.len(), 2);
    assert!(pair.event(panes(Some(7))).is_empty());
    assert!(pair
        .event(Event::CwdChanged(
            PaneId::Terminal(TerminalId::new(7)),
            "/work".into(),
            vec![]
        ))
        .is_empty());
    pair.event(Event::CwdChanged(
        PaneId::Terminal(TerminalId::new(7)),
        "/work/subdir".into(),
        vec![],
    ));
    pair.event(Event::CwdChanged(
        PaneId::Terminal(TerminalId::new(7)),
        "/missing".into(),
        vec![],
    ));
    pair.event(panes(None));
    pair.event(panes(Some(7)));
    pair.event(panes(Some(99)));
    assert_eq!(pair.wasm.render(20, 80), pair.steel.render(20, 80));
    for index in 0..501 {
        std::fs::write(
            pair.root.path().join("work/subdir").join(index.to_string()),
            "",
        )
        .unwrap();
    }
    let commands = pair.event(Event::CwdChanged(
        PaneId::Terminal(TerminalId::new(99)),
        "/work/subdir".into(),
        vec![],
    ));
    assert_eq!(
        commands.last().unwrap()["SetPaneRegexHighlights"][1]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn link_actual_wasm_and_steel_match_path_expansion_line_numbers_and_missing_files() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "link_actual_wasm_and_steel_match_path_expansion_line_numbers_and_missing_files",
    );
    let mut pair = Pair::new();
    for path in [
        "alpha.rs",
        "alpha.rs:42",
        "alpha.rs:42:10",
        "alpha.rs:1:2:3",
        "./alpha.rs",
        "café.txt:2",
        "~/alpha.rs",
        "$HOME/alpha.rs",
        "${PROJECT}/alpha.rs:9",
        " /work/alpha.rs ",
        "/work/alpha.rs:4294967296",
        "/work/alpha.rs:0",
        "$UNKNOWN",
        "~nobody",
    ] {
        let commands = pair.click(path, true);
        assert!(
            commands
                .iter()
                .any(|command| command.get("OpenFileFloating").is_some()),
            "{path}: {commands:?}"
        );
    }
    for path in ["/work", "~/subdir", "${PROJECT}/subdir", "subdir"] {
        assert!(pair
            .click(path, true)
            .iter()
            .any(|command| command.get("MessageToPlugin").is_some()));
    }
    for path in [
        "missing",
        "$UNSET/alpha.rs",
        "${unclosed/alpha.rs",
        "$9/alpha.rs",
        "alpha.rs:",
        "alpha.rs:2 ",
    ] {
        assert!(pair.click(path, true).is_empty(), "{path}");
    }
    assert!(pair.click("alpha.rs", false).is_empty());
    assert!(pair
        .click("/work/alpha.rs", false)
        .iter()
        .any(|command| command.get("OpenFileFloating").is_some()));
}

#[cfg(unix)]
#[test]
fn link_actual_wasm_and_steel_follow_file_directory_and_dangling_symlinks() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "link_actual_wasm_and_steel_follow_file_directory_and_dangling_symlinks",
    );
    let mut pair = Pair::new();
    for (target, name) in [
        ("alpha.rs", "file-link"),
        ("subdir", "dir-link"),
        ("missing", "dangling-link"),
    ] {
        std::os::unix::fs::symlink(target, pair.root.path().join("work").join(name)).unwrap();
    }
    pair.event(panes(Some(7)));
    assert!(pair
        .click("file-link:8", true)
        .iter()
        .any(|command| command.get("OpenFileFloating").is_some()));
    assert!(pair
        .click("dir-link", true)
        .iter()
        .any(|command| command.get("MessageToPlugin").is_some()));
    assert_eq!(pair.click("dangling-link", true), Vec::<Value>::new());
    assert_eq!(
        pair.click("/work/alpha.rs", false).last().unwrap()["OpenFileFloating"][0]["path"],
        json!("/work/alpha.rs")
    );
}
