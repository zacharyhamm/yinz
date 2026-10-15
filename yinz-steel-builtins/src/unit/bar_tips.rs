use crate::tests::{plugin_with_source, Host};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use yinz_steel::Runtime;
use yinz_types::data::ModeInfo;

const NAMES: [&str; 10] = [
    "quicknav",
    "floating_panes_mouse",
    "send_mouse_clicks_to_terminal",
    "move_focus_hjkl_tab_switch",
    "yinz_setup_check",
    "use_mouse",
    "sync_tab",
    "edit_scrollbuffer",
    "compact_layout",
    "move_tabs",
];

fn tips(cache: Option<String>) -> (Runtime, Arc<Mutex<Host>>) {
    let mut replies = BTreeMap::from([("GetYinzVersion".into(), json!("test"))]);
    if let Some(cache) = cache {
        replies.insert("FsMetadata".into(), json!({}));
        replies.insert("FsRead".into(), json!(cache.as_bytes()));
    }
    plugin_with_source(
        "status-bar",
        "(require \"./bar/tips.scm\")",
        json!({}),
        replies,
        "",
    )
}

fn written(host: &Arc<Mutex<Host>>) -> Option<Value> {
    let commands = &host.lock().unwrap().commands;
    let write = commands
        .iter()
        .rev()
        .find_map(|command| command.get("FsWrite"))?;
    assert_eq!(write["path"], "/tmp/status-bar-tips.cache");
    assert_eq!(write["append"], false);
    assert_eq!(write["create_dirs"], false);
    let bytes: Vec<u8> = serde_json::from_value(write["contents"].clone()).unwrap();
    Some(serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn missing_tip_cache_initializes_every_tip_and_prioritizes_quicknav() {
    let (mut runtime, host) = tips(None);
    assert_eq!(
        runtime.invoke("bar-cached-tip", vec![]).unwrap(),
        "quicknav"
    );
    let cache = written(&host).unwrap();
    assert_eq!(cache["yinz_version"], "test");
    assert_eq!(cache["cached_data"].as_object().unwrap().len(), NAMES.len());
    for name in NAMES {
        assert_eq!(
            cache["cached_data"][name],
            if name == "quicknav" { 2 } else { 1 }
        );
    }
    assert!(!host
        .lock()
        .unwrap()
        .commands
        .iter()
        .any(|command| command.get("FsRead").is_some()));
}

#[test]
fn cache_version_changes_reset_counts_and_existing_quicknav_reaches_threshold() {
    let old = json!({"yinz_version":"old", "cached_data":{"quicknav":999,"obsolete":4}});
    let (mut runtime, host) = tips(Some(old.to_string()));
    assert_eq!(
        runtime.invoke("bar-cached-tip", vec![]).unwrap(),
        "quicknav"
    );
    let cache = written(&host).unwrap();
    assert_eq!(cache["cached_data"]["quicknav"], 2);
    assert!(cache["cached_data"].get("obsolete").is_none());

    let current = json!({"yinz_version":"test", "cached_data":{"quicknav":20,"use_mouse":9}});
    let (mut runtime, host) = tips(Some(current.to_string()));
    assert_eq!(
        runtime.invoke("bar-cached-tip", vec![]).unwrap(),
        "quicknav"
    );
    assert_eq!(
        written(&host).unwrap()["cached_data"],
        json!({"quicknav":21,"use_mouse":9})
    );
}

#[test]
fn established_cache_selects_an_existing_tip_and_increments_only_its_count() {
    let counts: BTreeMap<_, _> = NAMES.into_iter().map(|name| (name, 21)).collect();
    let cache = json!({"yinz_version":"test", "cached_data":counts});
    let (mut runtime, host) = tips(Some(cache.to_string()));
    let selected = runtime.invoke("bar-cached-tip", vec![]).unwrap();
    let selected = selected.as_str().unwrap();
    assert!(NAMES.contains(&selected));
    let updated = written(&host).unwrap();
    for name in NAMES {
        assert_eq!(
            updated["cached_data"][name],
            if name == selected { 22 } else { 21 }
        );
    }
}

#[test]
fn corrupt_or_unavailable_cache_falls_back_without_overwriting_it() {
    for cache in [
        "{broken".to_owned(),
        json!({}).to_string(),
        json!({"yinz_version":"test","cached_data":[]}).to_string(),
        json!({"yinz_version":"test","cached_data":{"quicknav":-1}}).to_string(),
        json!({"yinz_version":"test","cached_data":{"quicknav":1.5}}).to_string(),
        json!({"yinz_version":"test","cached_data":{"quicknav":9007199254740992_u64}}).to_string(),
    ] {
        let (mut runtime, host) = tips(Some(cache));
        assert_eq!(
            runtime.invoke("bar-cached-tip", vec![]).unwrap(),
            "quicknav"
        );
        assert!(written(&host).is_none());
    }
    let (mut runtime, host) = tips(None);
    host.lock()
        .unwrap()
        .failures
        .insert("FsMetadata".into(), "permission denied".into());
    assert_eq!(
        runtime.invoke("bar-cached-tip", vec![]).unwrap(),
        "quicknav"
    );
    assert!(written(&host).is_none());
}

#[test]
fn every_tip_uses_a_fitting_form_and_short_viewports_render_nothing() {
    let (mut runtime, _) = tips(None);
    let mode = serde_json::to_value(ModeInfo::default()).unwrap();
    for name in NAMES {
        let full = runtime
            .invoke("bar-tip", vec![mode.clone(), json!(name), json!(1000)])
            .unwrap();
        assert!(!full["part"].as_str().unwrap().is_empty(), "{name}");
        let length = full["len"].as_u64().unwrap();
        for width in [0, 20, 60, length.saturating_sub(1), length] {
            let line = runtime
                .invoke("bar-tip", vec![mode.clone(), json!(name), json!(width)])
                .unwrap();
            assert!(
                line["len"].as_u64().unwrap() <= width,
                "{name} width {width}"
            );
            if width == 0 {
                assert_eq!(line["part"], "");
            }
        }
    }
}
