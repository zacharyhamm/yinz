use crate::tests::{plugin_with_replies, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_steel::Runtime;

pub(super) fn plugin(config: Value) -> (Runtime, Arc<Mutex<Host>>) {
    plugin_with_replies(
        "file-finder",
        config,
        BTreeMap::from([
            (
                "GetPluginIds".into(),
                json!({"plugin_id":42,"client_id":1,"yinz_pid":1234,"initial_cwd":"/work/project"}),
            ),
            (
                "GetFocusedPaneInfo".into(),
                json!({"tab_id":1,"pane_id":{"Terminal":7}}),
            ),
            ("GetPaneCwd".into(), json!("/work/project/src")),
            ("OpenCommandPaneOverlay".into(), json!({"Terminal":10})),
            ("OpenEditPaneInPlaceOfPaneId".into(), json!({"Terminal":11})),
            (
                "OpenCommandPaneInPlaceOfPaneId".into(),
                json!({"Terminal":12}),
            ),
        ]),
    )
}
fn call(runtime: &mut Runtime, name: &str, args: Vec<Value>) -> Value {
    runtime
        .invoke(name, args)
        .unwrap_or_else(|error| panic!("{name}: {error:#}"))
}
fn mount(runtime: &mut Runtime) {
    runtime.update(json!({"HostFolderChanged":"/"})).unwrap();
}
fn selected(runtime: &mut Runtime, host: &Arc<Mutex<Host>>, contents: &str) {
    host.lock()
        .unwrap()
        .replies
        .insert("FsRead".into(), json!(contents.as_bytes()));
    runtime
        .update(json!({"CommandPaneExited":[10,0,{}]}))
        .unwrap();
}
fn last(host: &Arc<Mutex<Host>>, name: &str) -> Value {
    host.lock()
        .unwrap()
        .commands
        .iter()
        .rev()
        .find_map(|command| command.get(name).cloned())
        .unwrap()
}

#[test]
fn finder_waits_for_host_mount_then_replaces_the_overlay_and_retires_the_editor() {
    let (mut runtime, host) = plugin(json!({}));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!({"ChangeHostFolder":"/"}))
    );
    assert!(!host
        .lock()
        .unwrap()
        .commands
        .iter()
        .any(|command| command.get("OpenCommandPaneOverlay").is_some()));
    mount(&mut runtime);
    let opened = last(&host, "OpenCommandPaneOverlay");
    assert_eq!(opened[0]["cwd"], "/host/work/project/src");
    assert!(opened[0]["args"][1]
        .as_str()
        .unwrap()
        .contains("git ls-files --cached --others --exclude-standard -z"));
    assert_eq!(opened[1]["kind"], "Dismissible");
    assert_eq!(opened[1]["coordinates"]["width"], json!({"Fixed":120}));
    assert_eq!(
        last(&host, "RenameTerminalPane"),
        json!([10, "Find file — src"])
    );
    selected(&mut runtime, &host, "/work/project/src/main.rs\n");
    assert_eq!(
        last(&host, "OpenEditPaneInPlaceOfPaneId"),
        json!([
            {"Terminal":10},{"path":"/host/work/project/src/main.rs","line_number":null,"cwd":"/host/work/project/src"},true,{}
        ])
    );
    assert_eq!(last(&host, "FocusTerminalPane"), json!([11, true, false]));
    runtime.update(json!({"EditPaneExited":[11,0,{}]})).unwrap();
    assert_eq!(last(&host, "CloseTerminalPane"), json!(11));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!("CloseSelf"))
    );
    assert_eq!(runtime.render(10, 80).unwrap(), "");
}

#[test]
fn finder_openers_validate_every_entry_and_preserve_literal_arguments() {
    let (mut runtime, _) = plugin(json!({}));
    let file = "/tmp/ space 'quote\",雪#?%2C{} .PDF";
    let structured = json!({"pdf":{"argv":["/opt/Image Tools/{}","","--title","Selected 'image'","before{}after{}"]}}).to_string();
    let result = call(
        &mut runtime,
        "ff-opener",
        vec![
            json!({"openers_json":structured,"open_pdf":"legacy"}),
            json!(file),
            json!(file),
        ],
    );
    assert_eq!(
        result,
        json!({"command":{"path":"/opt/Image Tools/{}","args":["","--title","Selected 'image'",format!("before{file}after{file}")]}})
    );
    for (configuration, file, expected) in [
        (
            json!({"open_pdf":"legacy","openers_json":"{\"pdf\":null}"}),
            "/a.pdf",
            json!("editor"),
        ),
        (
            json!({"open_pdf":"legacy","openers_json":"{\"default\":{\"argv\":[\"fallback\"]}}"}),
            "/a.PDF",
            json!({"command":{"path":"legacy","args":["/a.PDF"]}}),
        ),
        (
            json!({"open_default":"bat","open_gitignore":"never"}),
            "/p/.gitignore",
            json!({"command":{"path":"bat","args":["/p/.gitignore"]}}),
        ),
        (
            json!({"open_pdf":"   ","open_default":"bat"}),
            "/a.pdf",
            json!("editor"),
        ),
    ] {
        assert_eq!(
            call(
                &mut runtime,
                "ff-opener",
                vec![configuration, json!(file), json!(file)]
            ),
            expected
        );
    }
    for invalid in [
        "{",
        "null",
        "[]",
        r#"{"PDF":null}"#,
        r#"{".pdf":null}"#,
        r#"{"pdf":false}"#,
        r#"{"pdf":{"argv":[]}}"#,
        r#"{"pdf":{"argv":[" "]}}"#,
        r#"{"pdf":{"argv":["viewer",null]}}"#,
        r#"{"pdf":{"argv":["viewer","\u0000"]}}"#,
        r#"{"pdf":{"argv":["viewer"],"plugin":"reader"}}"#,
        r#"{"pdf":{"plugin":""}}"#,
        r#"{"pdf":{"plugin":"reader","message":null}}"#,
        r#"{"pdf":{"plugin_id":7,"config":null}}"#,
        r#"{"pdf":{"plugin_id":null,"plugin":"reader"}}"#,
        r#"{"pdf":{"plugin_id":-1}}"#,
        r#"{"pdf":{"plugin":"reader","delivery":"focus"}}"#,
    ] {
        let error = runtime
            .invoke(
                "ff-opener",
                vec![
                    json!({"openers_json":invalid,"open_txt":"fallback"}),
                    json!("/a.txt"),
                    json!("/a.txt"),
                ],
            )
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("openers_json"),
            "{invalid}: {error:#}"
        );
    }
}

#[test]
fn finder_plugin_destinations_keep_raw_paths_and_urls_distinct() {
    let (mut runtime, _) = plugin(json!({}));
    for (value, expected) in [
        ("/tmp/a%2C.scm", "file:///tmp/a%252C.scm"),
        (
            "/tmp/a '雪,#?%2C.scm",
            "file:///tmp/a%20'%E9%9B%AA,%23%3F%252C.scm",
        ),
        ("file:/tmp/a%2C.scm", "file:/tmp/a%2C.scm"),
        ("file://localhost/tmp/a.scm", "file://localhost/tmp/a.scm"),
        ("reader", "reader"),
        ("yinz:reader", "yinz:reader"),
    ] {
        assert_eq!(
            call(&mut runtime, "ff-plugin-destination", vec![json!(value)]),
            json!(expected)
        );
    }
    for value in [
        "./a.scm",
        "../a.scm",
        "a/main.scm",
        "file:a.scm",
        "https://example.com/a.scm",
        "file://remote/tmp/a.scm",
        "file:/a.scm?x",
        "file:/a.scm#x",
        "yinz:",
        "a\0",
    ] {
        assert!(
            runtime
                .invoke("ff-plugin-destination", vec![json!(value)])
                .is_err(),
            "{value:?}"
        );
    }
}

#[test]
fn finder_configuration_refresh_reader_reuse_and_focus_bounce_preserve_the_actor() {
    let (mut runtime, host) = plugin(json!({"open_pdf":"old"}));
    mount(&mut runtime);
    host.lock().unwrap().replies.insert(
        "GetFocusedPaneInfo".into(),
        json!({"pane_id":{"Plugin":42}}),
    );
    runtime.update(json!({"PaneUpdate":{}})).unwrap();
    assert_eq!(last(&host, "FocusTerminalPane"), json!([10, true, false]));
    runtime.update(json!({"PluginConfigurationChanged":{"openers_json":"{\"pdf\":{\"plugin\":\"reader\",\"args\":{\"path\":\"{}\"}}}"}})).unwrap();
    host.lock()
        .unwrap()
        .replies
        .insert("GetPluginIdsForUrlWithConfiguration".into(), json!([9, 3]));
    host.lock()
        .unwrap()
        .replies
        .insert("GetPluginInfo".into(), json!({"background":false}));
    selected(&mut runtime, &host, "/tmp/a.pdf\n");
    assert_eq!(
        last(&host, "MessageToPlugin"),
        json!({"destination_plugin_id":3,"message_name":"open","message_args":{"path":"{}"},"message_payload":"/tmp/a.pdf"})
    );
    assert_eq!(last(&host, "FocusPluginPane"), json!([3, true, false]));
    let commands = &host.lock().unwrap().commands;
    let close = commands
        .iter()
        .position(|c| c.get("CloseTerminalPane") == Some(&json!(10)))
        .unwrap();
    let focus = commands
        .iter()
        .position(|c| c.get("FocusPluginPane").is_some())
        .unwrap();
    assert!(close < focus);
}

#[test]
fn finder_cancel_and_structured_failures_report_once_and_clean_up() {
    for (contents, expected) in [
        ("", json!({"Ok":null})),
        ("/tmp/a\nfile\0", json!({"Ok":"/tmp/a\nfile"})),
        (
            "/a\0/b\0",
            json!({"Err":"finder must return exactly one absolute path"}),
        ),
    ] {
        let (mut runtime, host) = plugin(json!({}));
        host.lock().unwrap().replies.insert(
            "Pick".into(),
            json!({"options":{"kind":"file"},"cwd":"/tmp"}),
        );
        runtime.load(json!({})).unwrap();
        mount(&mut runtime);
        selected(&mut runtime, &host, contents);
        assert_eq!(last(&host, "Pick"), json!({"Complete":{"result":expected}}));
        assert!(!host
            .lock()
            .unwrap()
            .commands
            .iter()
            .any(|c| c.get("OpenEditPaneInPlaceOfPaneId").is_some()));
    }
    let (mut runtime, host) = plugin(json!({"listing_json":"{\"kind\":\"file\"}"}));
    mount(&mut runtime);
    runtime
        .update(json!({"CommandPaneExited":[10,11,{}]}))
        .unwrap();
    assert!(last(&host, "Notify")[0]
        .as_str()
        .unwrap()
        .contains("fzf is not installed"));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!("CloseSelf"))
    );
}

#[test]
fn finder_script_and_selection_rules_preserve_paths_and_validate_options() {
    let (mut runtime, _) = plugin(json!({}));
    let script = call(
        &mut runtime,
        "ff-script",
        vec![
            json!({"file_command":"rg --files --hidden","fzf_args":"--preview 'head -50 {}'"}),
            json!("/tmp/it's"),
        ],
    );
    let script = script.as_str().unwrap();
    assert!(script.contains("list() { rg --files --hidden; }"));
    assert!(script.contains("fzf --prompt 'file> ' --preview 'head -50 {}'"));
    assert!(script.contains("esac > '/tmp/it'\\''s'"));
    assert!(!script.contains("git ls-files"));
    for (contents, structured, expected) in [
        ("/a\r\n/b", false, json!("/a\r")),
        ("\n", false, Value::Null),
        ("   \n", false, json!("   ")),
        ("/a\nfile\0", true, json!("/a\nfile")),
    ] {
        assert_eq!(
            call(
                &mut runtime,
                "ff-selection",
                vec![json!(contents), json!(structured)]
            ),
            expected
        );
    }
    for options in [
        json!({"kind":"file","roots":[]}),
        json!({"kind":"file","roots":["a\0"]}),
        json!({"kind":"directory","extensions":[]}),
        json!({"kind":"file","extensions":["*.pdf"]}),
        json!({"kind":"file","hidden":1}),
    ] {
        assert!(runtime
            .invoke("ff-validate-options", vec![options])
            .is_err());
    }
}

#[test]
fn finder_project_titles_follow_worktree_gitdir_and_ignore_detached_heads() {
    let files = BTreeMap::from([
        ("/host/work/main/.git/HEAD", "ref: refs/heads/main\n"),
        (
            "/host/work/main/.git/worktrees/other/HEAD",
            "ref: refs/heads/feature/nested\n",
        ),
        (
            "/host/work/other/.git",
            "gitdir: /work/main/.git/worktrees/other\n",
        ),
        ("/host/work/detached/.git/HEAD", "abcdef1234\n"),
    ]);
    let mut runtime = Runtime::new(move |command: Value| {
        serde_json::from_value::<yinz_types::data::PluginCommand>(command.clone()).unwrap();
        if let Some(metadata) = command.get("FsMetadata") {
            let path = metadata["path"].as_str().unwrap();
            let file = files.contains_key(path);
            let dir = files
                .keys()
                .any(|entry| entry.starts_with(&format!("{path}/")));
            return Ok(if file || dir {
                json!({"is_dir":dir,"is_file":file,"is_symlink":false,"len":0})
            } else {
                Value::Null
            });
        }
        if let Some(read) = command.get("FsRead") {
            let path = read["path"].as_str().unwrap();
            return files
                .get(path)
                .map(|text| json!(text.as_bytes()))
                .ok_or_else(|| std::io::Error::other("missing").into());
        }
        panic!("unexpected project lookup {command}");
    })
    .unwrap();
    let source = r#"(require "./common.scm")"#;
    let mut root = yinz_steel::modules::SourceRoot::embedded(
        "main.scm",
        BTreeMap::from([
            ("main.scm".into(), source.into()),
            (
                "common.scm".into(),
                std::fs::read_to_string(crate::asset_dir().join("file-finder/common.scm")).unwrap(),
            ),
        ]),
    )
    .unwrap();
    runtime.eval_with_source_root(source, &mut root).unwrap();
    for (cwd, expected) in [
        ("/work/main/src", json!("main (main)")),
        ("/work/other/src", json!("other (feature/nested)")),
        ("/work/detached", json!("detached")),
        ("/downloads", json!("downloads")),
        ("/", json!(false)),
        (".", json!(false)),
    ] {
        assert_eq!(
            call(&mut runtime, "ff-project-label", vec![json!(cwd)]),
            expected,
            "{cwd}"
        );
    }
}

#[test]
fn finder_geometry_and_missing_reader_errors_do_not_leave_hidden_instances() {
    let (mut runtime, host) = plugin(json!({"width":"0", "height":"50%", "x":"101%", "y":"0"}));
    mount(&mut runtime);
    assert_eq!(
        last(&host, "OpenCommandPaneOverlay")[1]["coordinates"],
        json!({
            "width":null,"height":{"Percent":50},"x":null,"y":{"Fixed":0},"pinned":null,"borderless":null,
        })
    );
    runtime.update(json!({"PluginConfigurationChanged":{"openers_json":"{\"default\":{\"plugin\":\"reader\",\"delivery\":\"message\"}}"}})).unwrap();
    host.lock()
        .unwrap()
        .replies
        .insert("GetPluginIdsForUrlWithConfiguration".into(), json!([]));
    selected(&mut runtime, &host, "/tmp/a\n");
    assert!(last(&host, "Notify")[0]
        .as_str()
        .unwrap()
        .contains("message destination is not running"));
    assert_eq!(
        host.lock().unwrap().commands.last(),
        Some(&json!("CloseSelf"))
    );
}
