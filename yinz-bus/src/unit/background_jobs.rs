use super::*;
use std::os::unix::net::UnixListener;
use tempfile::tempdir;
use yinz_types::data::SessionInfo;

#[test]
fn checkpoint_rename_redirects_queued_saves_and_metadata() {
    let root = tempdir().unwrap();
    let writer = Arc::new(SessionCheckpointWriter {
        cache_dir: root.path().to_owned(),
        name: Mutex::new(None),
        progress: Mutex::default(),
        #[cfg(test)]
        before_write: None,
        #[cfg(test)]
        after_abort: None,
    });
    let old = SessionName::new("old");
    let renamed = SessionName::new("renamed");
    let snapshot = SessionInfo::new(old.clone());
    let periodic_writer = writer.clone();
    let explicit_writer = writer.clone();
    writer
        .write(
            old.clone(),
            snapshot.clone(),
            &("original".into(), BTreeMap::new()),
        )
        .unwrap();
    writer.rename(&old, &renamed).unwrap();

    for (queued_writer, contents) in [(periodic_writer, "periodic"), (explicit_writer, "explicit")]
    {
        queued_writer
            .write(
                old.clone(),
                snapshot.clone(),
                &(contents.into(), BTreeMap::new()),
            )
            .unwrap();
        assert!(
            !root.path().join(&old).exists(),
            "a stale snapshot recreated the old session"
        );
        let checkpoint =
            yinz_runtime::session_checkpoint::SessionCheckpoint::read(&root.path().join(&renamed))
                .unwrap();
        let metadata = fs::read_to_string(checkpoint.metadata_path()).unwrap();
        assert_eq!(
            yinz_session::metadata::session_info::from_string(&metadata, &renamed)
                .unwrap()
                .name,
            renamed
        );
        assert_eq!(
            fs::read_to_string(checkpoint.layout_path()).unwrap(),
            contents
        );
    }
}

#[test]
fn checkpoint_rename_before_first_save_and_failed_rename_keep_the_right_owner() {
    let root = tempdir().unwrap();
    let writer = SessionCheckpointWriter {
        cache_dir: root.path().to_owned(),
        name: Mutex::new(None),
        progress: Mutex::default(),
        #[cfg(test)]
        before_write: None,
        #[cfg(test)]
        after_abort: None,
    };
    let old = SessionName::new("old");
    let renamed = SessionName::new("renamed");
    let blocked = SessionName::new("blocked");
    let layout = ("layout {}".into(), BTreeMap::new());
    writer.rename(&old, &renamed).unwrap();
    writer
        .write(old.clone(), SessionInfo::new(old.clone()), &layout)
        .unwrap();
    assert!(!root.path().join(&old).exists());

    fs::write(root.path().join(&blocked), "existing data").unwrap();
    assert!(writer.rename(&renamed, &blocked).is_err());
    assert_eq!(*writer.name.lock().unwrap(), Some(renamed.clone()));
    writer
        .write(old.clone(), SessionInfo::new(old.clone()), &layout)
        .unwrap();
    assert!(!root.path().join(&old).exists());
    assert!(root.path().join(&renamed).is_dir());
    assert_eq!(
        fs::read_to_string(root.path().join(&blocked)).unwrap(),
        "existing data"
    );
}

fn make_socket(dir: &std::path::Path, name: &str) -> UnixListener {
    UnixListener::bind(dir.join(name)).expect("bind unix socket")
}

/// The legacy on-disk shape: bare files in the session folder, no `.checkpoint.lock` and no
/// `current` pointer. Only `scan_session_list_reads_the_legacy_flat_files` should use these
/// two; everything a running server writes goes through `write_checkpoint`.
fn write_flat_metadata(info_dir: &std::path::Path, session: &str, info: &SessionInfo) {
    let folder = info_dir.join(session);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("session-metadata.kdl"),
        yinz_session::metadata::session_info::to_string(info).unwrap(),
    )
    .unwrap();
}

fn write_flat_layout(info_dir: &std::path::Path, session: &str) {
    let folder = info_dir.join(session);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("session-layout.kdl"), "layout { }").unwrap();
}

/// Publishes a session the way `SessionCheckpointWriter` does: metadata and layout
/// under a `.checkpoint-*` directory behind a `current` symlink.
fn publish(info_dir: &std::path::Path, session: &str) {
    yinz_runtime::session_checkpoint::write_checkpoint(
        &info_dir.join(session),
        &yinz_session::metadata::session_info::to_string(&SessionInfo::new(
            session.to_owned().into(),
        ))
        .unwrap(),
        &("layout {}".to_owned(), BTreeMap::new()),
    )
    .unwrap();
}

fn scan(
    current: &str,
    sock_dir: &std::path::Path,
    info_dir: &std::path::Path,
) -> (
    BTreeMap<SessionName, SessionInfo>,
    BTreeMap<SessionName, Duration>,
) {
    scan_session_list(
        &SessionName::new(current),
        &[],
        &BTreeMap::new(),
        sock_dir,
        info_dir,
    )
}

#[test]
fn scan_session_list_reads_published_checkpoints() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    let _listener = make_socket(sock_dir.path(), "live");
    for name in ["live", "dead"] {
        publish(info_dir.path(), name);
    }
    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert_eq!(
        live.keys().map(SessionName::as_str).collect::<Vec<_>>(),
        vec!["live"]
    );
    assert_eq!(
        resurrectable
            .keys()
            .map(SessionName::as_str)
            .collect::<Vec<_>>(),
        vec!["dead"]
    );
}

#[test]
fn scan_session_list_returns_empty_when_no_peers() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert!(live.is_empty());
    assert!(resurrectable.is_empty());
}

#[test]
fn scan_session_list_finds_peer_from_socket_and_metadata() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    let peer = "peer-alpha";
    let _listener = make_socket(sock_dir.path(), peer);
    publish(info_dir.path(), peer);

    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert_eq!(live.len(), 1);
    assert!(live.contains_key(peer));
    assert!(resurrectable.is_empty());
}

/// Only the current session's entry is enriched with the plugin list and the layouts this
/// server knows about; a peer's entry is left as its metadata describes it.
#[test]
fn scan_session_list_enriches_only_the_current_session() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    let _listeners: Vec<UnixListener> = ["me", "other"]
        .into_iter()
        .map(|name| {
            publish(info_dir.path(), name);
            make_socket(sock_dir.path(), name)
        })
        .collect();
    let layouts = vec![LayoutInfo::BuiltIn("compact".to_owned())];
    let plugin = RunPlugin {
        location: yinz_types::input::layout::RunPluginLocation::File(PathBuf::from(
            "/plugins/status.scm",
        )),
        ..Default::default()
    };
    let plugins = BTreeMap::from([(PluginId::new(7), plugin.clone())]);

    let (live, _) = scan_session_list(
        &SessionName::new("me"),
        &layouts,
        &plugins,
        sock_dir.path(),
        info_dir.path(),
    );

    let me = &live[&SessionName::new("me")];
    assert_eq!(me.available_layouts, layouts);
    assert_eq!(
        me.plugins,
        BTreeMap::from([(PluginId::new(7), yinz_types::data::PluginInfo::from(plugin))])
    );
    let other = &live[&SessionName::new("other")];
    assert!(other.available_layouts.is_empty(), "{other:?}");
    assert!(other.plugins.is_empty(), "{other:?}");
}

/// A dead session is resurrectable when its layout is on disk; nothing else about it is
/// consulted. The reported age is how long ago that layout was written.
#[test]
fn scan_session_list_finds_resurrectable_from_orphan_layout() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    publish(info_dir.path(), "dead-beta");

    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert!(live.is_empty());
    assert_eq!(
        resurrectable
            .keys()
            .map(SessionName::as_str)
            .collect::<Vec<_>>(),
        vec!["dead-beta"]
    );
    let dead_for = resurrectable[&SessionName::new("dead-beta")];
    assert!(
        dead_for < Duration::from_secs(60),
        "the layout was just written, so the session died {dead_for:?} ago at most"
    );
}

/// Metadata alone is not enough to bring a session back, so a folder with no layout is left
/// off the list rather than offered as resurrectable.
#[test]
fn scan_session_list_drops_a_folder_with_metadata_but_no_layout() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    write_flat_metadata(
        info_dir.path(),
        "no-layout",
        &SessionInfo::new("no-layout".to_owned().into()),
    );

    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert!(live.is_empty());
    assert!(resurrectable.is_empty(), "{resurrectable:?}");
}

/// The pre-checkpoint layout: bare `session-metadata.kdl` / `session-layout.kdl` in the
/// session folder, which `SessionCheckpoint::read` still accepts for sessions written by
/// an older server.
#[test]
fn scan_session_list_reads_the_legacy_flat_files() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    let _listener = make_socket(sock_dir.path(), "live");
    write_flat_metadata(
        info_dir.path(),
        "live",
        &SessionInfo::new("live".to_owned().into()),
    );
    write_flat_layout(info_dir.path(), "dead");

    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert_eq!(
        live.keys().map(SessionName::as_str).collect::<Vec<_>>(),
        vec!["live"]
    );
    assert_eq!(
        resurrectable
            .keys()
            .map(SessionName::as_str)
            .collect::<Vec<_>>(),
        vec!["dead"]
    );
}

/// Stale sockets stay on disk but their checkpoints are offered for resurrection.
#[test]
fn scan_session_list_separates_live_from_resurrectable() {
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    let _listeners: Vec<UnixListener> = ["live-a", "live-b", "live-c"]
        .into_iter()
        .map(|name| {
            publish(info_dir.path(), name);
            make_socket(sock_dir.path(), name)
        })
        .collect();
    for name in ["dead-a", "dead-b"] {
        publish(info_dir.path(), name);
    }
    drop(make_socket(sock_dir.path(), "dead-b"));

    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert_eq!(
        live.keys().map(SessionName::as_str).collect::<Vec<_>>(),
        vec!["live-a", "live-b", "live-c"]
    );
    assert_eq!(
        resurrectable
            .keys()
            .map(SessionName::as_str)
            .collect::<Vec<_>>(),
        vec!["dead-a", "dead-b"]
    );
    assert!(sock_dir.path().join("dead-b").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn scan_session_list_keeps_a_full_backlog_live() {
    use std::os::fd::AsRawFd;
    let sock_dir = tempdir().unwrap();
    let info_dir = tempdir().unwrap();
    publish(info_dir.path(), "busy");
    let listener = make_socket(sock_dir.path(), "busy");
    // Safety: the listener owns this socket; only its backlog limit changes.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
    let path = sock_dir.path().join("busy");
    let _first = yinz_ipc::consts::ipc_connect(&path).unwrap();
    let _second = yinz_ipc::consts::ipc_connect(&path).unwrap();
    let (live, resurrectable) = scan("me", sock_dir.path(), info_dir.path());
    assert!(live.contains_key(&SessionName::new("busy")));
    assert!(resurrectable.is_empty());
    assert!(path.exists());
}
