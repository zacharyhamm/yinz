use super::runtime_parity::json_reply;
use super::runtime_parity::{default_reply, Responder, SteelReference, WasmReference};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use yinz_types::{
    data::{Event, PaneId, PluginCommand},
    internal_ids::TerminalId,
};

fn host(root: &Path) -> Responder {
    let root = root.to_owned();
    Arc::new(move |command| {
        Some(match command {
            PluginCommand::GetFocusedPaneInfo => {
                json_reply(json!({"tab_id":1,"pane_id":{"Terminal":7}}))
            },
            PluginCommand::GetPaneCwd { .. } => json_reply("/work/project/src"),
            PluginCommand::Pick(_) => json_reply(()),
            PluginCommand::OpenCommandPaneOverlay(..) => json_reply(json!({"Terminal":10})),
            PluginCommand::OpenEditPaneInPlaceOfPaneId(..) => json_reply(json!({"Terminal":11})),
            PluginCommand::OpenCommandPaneInPlaceOfPaneId(..) => json_reply(json!({"Terminal":12})),
            PluginCommand::FsMetadata { .. } => json_reply(()),
            PluginCommand::FsRead { path, .. } => {
                let path = root.join(path.strip_prefix("/host").unwrap());
                std::fs::read(path)
                    .map(crate::response::DecodedResponse::Bytes)
                    .map_err(|e| e.to_string())
            },
            PluginCommand::FsRemove { path, .. } => {
                let path = root.join(path.strip_prefix("/host").unwrap());
                let _ = std::fs::remove_file(path);
                json_reply(true)
            },
            _ => return default_reply(command),
        })
    })
}
fn comparable(commands: Vec<Value>) -> Vec<Value> {
    commands
        .into_iter()
        .filter(|command| {
            // The retained Wasm reference performs these same operations through WASI.
            !["FsRead", "FsMetadata", "FsRemove"]
                .iter()
                .any(|name| command.get(name).is_some())
        })
        .map(|mut command| {
            // The finder now retains candidates when an unreadable descendant makes find fail.
            // Listing behavior is exercised by file_finder_listing; the rest of the script stays exact.
            if let Some(script) = command.pointer_mut("/OpenCommandPaneOverlay/0/args/1")
                && let Some(text) = script.as_str()
            {
                *script = Value::String(
                    text.replace("&& { find", "&& find")
                        .replace(" || [ -s \"$candidates\" ]; }", ""),
                );
            }
            command
        })
        .collect()
}

#[test]
fn file_finder_actual_wasm_and_steel_match_overlay_scripts_selection_and_retirement() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "file_finder_actual_wasm_and_steel_match_overlay_scripts_selection_and_retirement",
    );
    for config in [
        BTreeMap::new(),
        BTreeMap::from([("file_command".into(),"rg --files --hidden".into()),("fzf_args".into(),"--preview 'head -50 {}'".into()),("open_md".into(),"glow -p".into())]),
        BTreeMap::from([("listing_json".into(),json!({"kind":"file","roots":["a'[]\\\n"],"hidden":false,"recursive":false,"extensions":["md"]}).to_string())]),
        BTreeMap::from([("x".into(), "invalid".into()), ("y".into(), "101%".into()), ("width".into(), "0".into()), ("height".into(), "-1".into())]),
    ] {
        let root=tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("tmp")).unwrap();
        std::fs::create_dir_all(root.path().join("work/project/src")).unwrap();
        let responder=host(root.path());
        let mut wasm=WasmReference::new("file-finder",config.clone(),root.path(),responder.clone());
        let mut steel=SteelReference::new("file-finder",config.clone(),root.path(),responder);
        assert_eq!(comparable(wasm.take_commands()),comparable(steel.take_commands()),"load");
        assert_eq!(wasm.render(20,80),steel.render(20,80));
        let event=Event::HostFolderChanged("/".into());
        assert_eq!(wasm.update(&event),steel.update(&event));
        assert_eq!(comparable(wasm.take_commands()),comparable(steel.take_commands()),"finder overlay and listing script");
        let selection=if config.contains_key("listing_json") {"/work/project/book.md\0"} else {"/work/project/book.md\n"};
        let exit=Event::CommandPaneExited(TerminalId::new(10),Some(0),BTreeMap::new());
        std::fs::write(root.path().join("tmp/yinz-file-finder-1234-1.sel"),selection).unwrap();
        assert!(!wasm.update(&exit));
        std::fs::write(root.path().join("tmp/yinz-file-finder-1234-1.sel"),selection).unwrap();
        assert!(!steel.update(&exit));
        let old=comparable(wasm.take_commands());
        let new=comparable(steel.take_commands());
        assert_eq!(old,new,"selected opener");
        let id=if config.contains_key("open_md") {12} else {11};
        let closed=Event::PaneClosed(PaneId::Terminal(TerminalId::new(id)));
        wasm.update(&closed);steel.update(&closed);
        assert_eq!(comparable(wasm.take_commands()),comparable(steel.take_commands()),"close cleanup");
    }
}

fn commands(wasm: &WasmReference, steel: &SteelReference, phase: &str) {
    assert_eq!(
        comparable(wasm.take_commands()),
        comparable(steel.take_commands()),
        "{phase}"
    );
}
fn selection(
    root: &Path,
    wasm: &mut WasmReference,
    steel: &mut SteelReference,
    contents: &str,
    code: i32,
) {
    let event = Event::CommandPaneExited(TerminalId::new(10), Some(code), BTreeMap::new());
    let path = root.join("tmp/yinz-file-finder-1234-1.sel");
    std::fs::write(&path, contents).unwrap();
    assert!(!wasm.update(&event));
    std::fs::write(&path, contents).unwrap();
    assert!(!steel.update(&event));
    commands(wasm, steel, "selection result");
}

#[test]
fn file_finder_actual_runtimes_match_picker_success_cancel_errors_and_mount_failure() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "file_finder_actual_runtimes_match_picker_success_cancel_errors_and_mount_failure",
    );
    for (contents, code, mount) in [
        ("/work/a \n雪\0", 0, true),
        ("", 0, true),
        ("", 10, true),
        ("", 11, true),
        ("", 2, true),
        ("", 0, false),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("tmp")).unwrap();
        let base = host(root.path());
        let responder: Responder = Arc::new(move |command| {
            match command {
            PluginCommand::Pick(yinz_types::pick::PickCommand::Request) => Some(json_reply(serde_json::from_value::<yinz_types::pick::PickRequest>(json!({"options":{"kind":"file","roots":null,"extensions":null,"hidden":false,"timeoutMs":null},"cwd":"/work"})).unwrap())),
            _ => base(command),
        }
        });
        let mut wasm = WasmReference::new(
            "file-finder",
            BTreeMap::new(),
            root.path(),
            responder.clone(),
        );
        let mut steel = SteelReference::new("file-finder", BTreeMap::new(), root.path(), responder);
        commands(&wasm, &steel, "picker load");
        let event = if mount {
            Event::HostFolderChanged("/".into())
        } else {
            Event::FailedToChangeHostFolder(Some("unavailable".into()))
        };
        assert_eq!(wasm.update(&event), steel.update(&event));
        commands(&wasm, &steel, "picker mount");
        if mount {
            selection(root.path(), &mut wasm, &mut steel, contents, code);
        }
    }
}

#[test]
fn file_finder_actual_runtimes_match_reader_reuse_launch_message_and_missing_reader() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "file_finder_actual_runtimes_match_reader_reuse_launch_message_and_missing_reader",
    );
    for (ids, background, delivery, configuration) in [
        (vec![8, 3], false, "auto", json!({"theme":"dark"})),
        (vec![8, 3], true, "auto", json!({})),
        (vec![8, 3], false, "message", json!({})),
        (vec![], false, "auto", json!({})),
        (vec![], false, "message", json!({})),
        (vec![], false, "auto", Value::Null),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("tmp")).unwrap();
        let base = host(root.path());
        let responder: Responder = Arc::new(move |command| {
            Some(match command {
                PluginCommand::GetPluginIdsForUrlWithConfiguration { .. } => {
                    json_reply(ids.clone())
                },
                PluginCommand::GetPluginInfo(..) => {
                    json_reply(json!({"pluginId":3,"background":background}))
                },
                PluginCommand::OpenPluginPaneFloating { .. } => json_reply(json!({"Plugin":9})),
                _ => return base(command),
            })
        });
        let config = BTreeMap::from([("openers_json".into(), json!({"md":{"plugin":"reader","config":configuration,"delivery":delivery,"message":"view","args":{"literal":"{}"}}}).to_string())]);
        let mut wasm = WasmReference::new(
            "file-finder",
            config.clone(),
            root.path(),
            responder.clone(),
        );
        let mut steel = SteelReference::new("file-finder", config, root.path(), responder);
        commands(&wasm, &steel, "reader load");
        let event = Event::HostFolderChanged("/".into());
        assert_eq!(wasm.update(&event), steel.update(&event));
        commands(&wasm, &steel, "reader mount");
        selection(
            root.path(),
            &mut wasm,
            &mut steel,
            "/work/a '雪 #{}.md\n",
            0,
        );
    }
}

#[test]
fn file_finder_actual_runtimes_match_failed_overlay_and_editor_open() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "file_finder_actual_runtimes_match_failed_overlay_and_editor_open",
    );
    for fail_overlay in [true, false] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("tmp")).unwrap();
        let base = host(root.path());
        let responder: Responder = Arc::new(move |command| match command {
            PluginCommand::OpenCommandPaneOverlay(..) if fail_overlay => Some(json_reply(())),
            PluginCommand::OpenEditPaneInPlaceOfPaneId(..) if !fail_overlay => Some(json_reply(())),
            _ => base(command),
        });
        let mut wasm = WasmReference::new(
            "file-finder",
            BTreeMap::new(),
            root.path(),
            responder.clone(),
        );
        let mut steel = SteelReference::new("file-finder", BTreeMap::new(), root.path(), responder);
        commands(&wasm, &steel, "open failure load");
        let event = Event::HostFolderChanged("/".into());
        assert_eq!(wasm.update(&event), steel.update(&event));
        commands(&wasm, &steel, "overlay failure");
        if !fail_overlay {
            selection(root.path(), &mut wasm, &mut steel, "/work/file.md\n", 0);
        }
    }
}

#[test]
fn file_finder_actual_runtimes_reject_non_utf8_selection_files() {
    let _reference_trace = super::reference_trace::Scenario::new(
        "file_finder_actual_runtimes_reject_non_utf8_selection_files",
    );
    for structured in [false, true] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("tmp")).unwrap();
        let config = if structured {
            BTreeMap::from([("listing_json".into(), json!({"kind":"file"}).to_string())])
        } else {
            BTreeMap::new()
        };
        let responder = host(root.path());
        let mut wasm = WasmReference::new(
            "file-finder",
            config.clone(),
            root.path(),
            responder.clone(),
        );
        let mut steel = SteelReference::new("file-finder", config, root.path(), responder);
        commands(&wasm, &steel, "invalid UTF-8 load");
        let mount = Event::HostFolderChanged("/".into());
        wasm.update(&mount);
        steel.update(&mount);
        commands(&wasm, &steel, "invalid UTF-8 mount");
        let path = root.path().join("tmp/yinz-file-finder-1234-1.sel");
        let exited = Event::CommandPaneExited(TerminalId::new(10), Some(0), BTreeMap::new());
        std::fs::write(&path, [b'/', 255, 0]).unwrap();
        wasm.update(&exited);
        std::fs::write(&path, [b'/', 255, 0]).unwrap();
        steel.update(&exited);
        commands(&wasm, &steel, "invalid UTF-8 selection");
    }
}
