use crate::tests::{plugin_with_replies, rendered_text, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_steel::Runtime;
use yinz_types::data::{Event, PipeMessage};

fn plugin(config: Value) -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_replies(
        "strider",
        config,
        BTreeMap::from([("GetPluginIds".into(), json!({"initial_cwd":"/work"}))]),
    )
}

fn event(runtime: &mut Runtime, event: Value) -> bool {
    serde_json::from_value::<Event>(event.clone()).unwrap();
    runtime.update(event).unwrap()
}

fn key(runtime: &mut Runtime, bare: Value, modifiers: &[&str]) -> bool {
    event(
        runtime,
        json!({"Key":{"bare_key":bare,"key_modifiers":modifiers}}),
    )
}

fn character(runtime: &mut Runtime, text: &str) {
    for character in text.chars() {
        assert!(key(runtime, json!({"Char":character.to_string()}), &[]));
    }
}

fn entry(path: &str, dir: bool, len: u64) -> Value {
    json!([path,{"is_dir":dir,"is_file":!dir,"is_symlink":false,"len":len}])
}

fn files(runtime: &mut Runtime) {
    assert!(event(
        runtime,
        json!({"FileSystemUpdate":[
            entry("/host/zebra.txt",false,1024), entry("/host/docs",true,0),
            entry("/host/alpha.rs",false,13000), entry("/host/.env",false,22),
            ["/host/link",{"is_dir":true,"is_file":false,"is_symlink":true,"len":0}],
            ["/host/unknown",null], entry("/host/日本語.txt",false,0)
        ]})
    ));
}

fn commands(host: &Arc<Mutex<Host>>) -> Vec<Value> {
    host.lock().unwrap().commands.clone()
}

fn pipe(runtime: &mut Runtime, source: Value, args: Value, private: bool) -> bool {
    let message = json!({"source":source,"name":"filepicker","payload":null,"args":args,"is_private":private});
    serde_json::from_value::<PipeMessage>(message.clone()).unwrap();
    runtime.pipe(message).unwrap()
}

#[test]
fn load_respects_initial_and_caller_directory_and_host_folder_refresh() {
    let (mut runtime, host) = plugin(json!({}));
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ScanHostFolder":"/host"}))
    );
    assert!(commands(&host).iter().any(|c| c.get("Subscribe").is_some()));
    assert!(event(
        &mut runtime,
        json!({"HostFolderChanged":"/work/docs"})
    ));
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ScanHostFolder":"/host"}))
    );
    let (_, host) = plugin(json!({"caller_cwd":"/elsewhere"}));
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ChangeHostFolder":"/elsewhere"}))
    );
}

#[test]
fn directory_scan_sorts_folders_first_hides_dotfiles_and_skips_symlinks() {
    let (mut runtime, _) = plugin(json!({}));
    files(&mut runtime);
    let output = rendered_text(&runtime.render(20, 80).unwrap());
    assert!(output.find("docs/").unwrap() < output.find("alpha.rs").unwrap());
    assert!(output.find("alpha.rs").unwrap() < output.find("zebra.txt").unwrap());
    assert!(output.contains("1.02 kB"));
    assert!(output.contains("13 kB"));
    assert!(output.contains("unknown"));
    assert!(output.contains("日本語.txt"));
    assert!(!output.contains(".env"));
    assert!(!output.contains("link"));
    let (mut runtime, host) = plugin(json!({"show_hidden_files":"true"}));
    files(&mut runtime);
    assert!(rendered_text(&runtime.render(20, 80).unwrap()).contains(".env"));
    assert!(key(&mut runtime, json!({"Char":"e"}), &["Ctrl"]));
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ChangeHostFolder":"/work"}))
    );
    files(&mut runtime);
    assert!(!rendered_text(&runtime.render(20, 80).unwrap()).contains(".env"));
}

#[test]
fn keyboard_traverses_directories_parent_root_and_opens_current_directory() {
    let (mut runtime, host) = plugin(json!({}));
    files(&mut runtime);
    key(&mut runtime, json!("Up"), &[]);
    assert_eq!(runtime.value("selected").unwrap(), Value::Null);
    key(&mut runtime, json!("Down"), &[]);
    key(&mut runtime, json!("Right"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ChangeHostFolder":"/work/docs"}))
    );
    assert_eq!(runtime.value("selected").unwrap(), Value::Null);
    key(&mut runtime, json!("Backspace"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ChangeHostFolder":"/work"}))
    );
    character(&mut runtime, "..");
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ChangeHostFolder":"/"}))
    );
    character(&mut runtime, "/");
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"ChangeHostFolder":"/work"}))
    );
    files(&mut runtime);
    key(&mut runtime, json!("Down"), &[]);
    key(&mut runtime, json!("Esc"), &[]);
    key(&mut runtime, json!("Enter"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"OpenTerminal":{"path":"/work"}}))
    );
}

#[test]
fn fuzzy_search_selects_files_and_edits_unicode_without_losing_directory() {
    let (mut runtime, host) = plugin(json!({}));
    files(&mut runtime);
    character(&mut runtime, "日本");
    key(&mut runtime, json!("Backspace"), &[]);
    assert_eq!(runtime.value("search-term").unwrap(), "日");
    let output = rendered_text(&runtime.render(20, 80).unwrap());
    assert!(output.contains("日本語.txt"));
    assert!(!output.contains("alpha.rs"));
    key(&mut runtime, json!({"Char":"c"}), &["Ctrl"]);
    assert_eq!(runtime.value("search-term").unwrap(), "");
    character(&mut runtime, "ars");
    let output = rendered_text(&runtime.render(20, 80).unwrap());
    assert!(output.contains("alpha.rs"));
    assert!(!output.contains("zebra.txt"));
    key(&mut runtime, json!("Tab"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"OpenFile":[{"path":"/work/alpha.rs","cwd":"/work"},{}]}))
    );
    assert_eq!(runtime.value("search-term").unwrap(), "");
}

#[test]
fn existing_search_results_remain_until_the_next_search_edit() {
    let (mut runtime, _) = plugin(json!({}));
    files(&mut runtime);
    character(&mut runtime, "txt");
    key(&mut runtime, json!("Down"), &[]);
    key(&mut runtime, json!("Down"), &[]);
    assert_eq!(runtime.value("search-selected").unwrap(), 1);
    event(
        &mut runtime,
        json!({"FileSystemUpdate":[entry("/host/new.txt",false,4)]}),
    );
    let output = rendered_text(&runtime.render(20, 80).unwrap());
    assert!(!output.contains("new.txt"));
    assert!(output.contains("zebra.txt"));
    assert_eq!(runtime.value("search-selected").unwrap(), 1);
    key(&mut runtime, json!("Esc"), &[]);
    key(&mut runtime, json!("Down"), &[]);
    key(&mut runtime, json!("Down"), &[]);
    assert_eq!(runtime.value("selected").unwrap(), 0);
}

#[test]
fn standalone_replaces_plugin_for_file_and_directory_selection() {
    let (mut runtime, host) = plugin(json!({"close_on_selection":"true"}));
    key(&mut runtime, json!("Enter"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"OpenTerminalInPlaceOfPlugin":[{"path":"/work"},true]}))
    );
    let (mut runtime, host) = plugin(json!({}));
    assert!(pipe(
        &mut runtime,
        json!({"Plugin":3}),
        json!({"open_directly":"true"}),
        true
    ));
    files(&mut runtime);
    character(&mut runtime, "alpha");
    key(&mut runtime, json!("Enter"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"OpenFileInPlaceOfPlugin":[{"path":"/work/alpha.rs","cwd":"/work"},true,{}]}))
    );
}

#[test]
fn plugin_filepicker_returns_correlated_path_and_closes() {
    let (mut runtime, host) = plugin(json!({}));
    assert!(!pipe(&mut runtime, json!({"Plugin":9}), json!({}), false));
    assert!(pipe(
        &mut runtime,
        json!({"Plugin":9}),
        json!({"request":"abc"}),
        true
    ));
    assert!(rendered_text(&runtime.render(20, 100).unwrap()).contains("Select"));
    files(&mut runtime);
    character(&mut runtime, "alpha");
    key(&mut runtime, json!("Enter"), &[]);
    assert!(commands(&host).ends_with(&[
        json!({"MessageToPlugin":{"destination_plugin_id":9,"message_name":"filepicker_result","message_payload":"/work/alpha.rs","message_args":{"request":"abc"}}}),
        json!("CloseSelf")
    ]));
}

#[test]
fn cli_filepicker_blocks_then_returns_and_unblocks_without_opening() {
    let (mut runtime, host) = plugin(json!({}));
    assert!(pipe(
        &mut runtime,
        json!({"Cli":"pipe-id"}),
        json!({}),
        true
    ));
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"BlockCliPipeInput":"pipe-id"}))
    );
    key(&mut runtime, json!("Enter"), &[]);
    assert!(commands(&host).ends_with(&[
        json!({"CliPipeOutput":["pipe-id","/work"]}),
        json!({"UnblockCliPipeInput":"pipe-id"}),
        json!("CloseSelf")
    ]));
}

#[test]
fn mouse_hit_testing_tracks_scrolled_rows_and_ignores_headers_and_empty_space() {
    let (mut runtime, host) = plugin(json!({}));
    let entries: Vec<_> = (0..20)
        .map(|n| entry(&format!("/host/file{n:02}.txt"), false, n))
        .collect();
    event(&mut runtime, json!({"FileSystemUpdate":entries}));
    runtime.render(10, 80).unwrap();
    assert!(!event(&mut runtime, json!({"Mouse":{"LeftClick":[0,0]}})));
    assert!(!event(&mut runtime, json!({"Mouse":{"LeftClick":[-1,0]}})));
    assert!(!event(&mut runtime, json!({"Mouse":{"Hover":[9,0]}})));
    assert_eq!(runtime.value("selected").unwrap(), Value::Null);
    for _ in 0..12 {
        event(&mut runtime, json!({"Mouse":{"ScrollDown":1}}));
    }
    runtime.render(10, 80).unwrap();
    event(&mut runtime, json!({"Mouse":{"Hover":[4,0]}}));
    // The original pointer handler centers a five-row window around the selection.
    assert_eq!(runtime.value("selected").unwrap(), 9);
    runtime.render(10, 80).unwrap();
    event(&mut runtime, json!({"Mouse":{"LeftClick":[4,0]}}));
    assert_eq!(runtime.value("selected").unwrap(), 7);
    assert!(!commands(&host)
        .iter()
        .any(|command| command.get("OpenFile").is_some()));
}

#[test]
fn narrow_and_empty_viewports_render_without_invalid_style_indices() {
    let (mut runtime, _) = plugin(json!({"caller_cwd":"/長いパス/フォルダー/資料"}));
    files(&mut runtime);
    for rows in [0, 1, 2, 3, 4, 7, 8, 20] {
        for cols in [0, 1, 4, 10, 26, 57, 79, 100] {
            let output = runtime.render(rows, cols).unwrap();
            if rows == 0 || cols == 0 {
                assert!(output.is_empty());
            } else {
                assert!(rendered_text(&output).contains("FIND:"));
            }
        }
    }
}

#[test]
fn filenames_render_as_one_safe_line_but_open_the_original_path() {
    let (mut runtime, host) = plugin(json!({}));
    let name = "new\nline\t\u{1b}[31m.txt";
    event(
        &mut runtime,
        json!({"FileSystemUpdate":[entry(&format!("/host/{name}"),false,1)]}),
    );
    let output = rendered_text(&runtime.render(20, 80).unwrap());
    assert!(output.contains("new line    .txt"), "{output:?}");
    assert!(!output.contains("\u{1b}"));
    key(&mut runtime, json!("Down"), &[]);
    key(&mut runtime, json!("Enter"), &[]);
    assert_eq!(
        commands(&host).last(),
        Some(&json!({"OpenFile":[{"path":format!("/work/{name}"),"cwd":"/work"},{}]}))
    );
}
