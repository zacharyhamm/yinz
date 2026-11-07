use super::*;
use yinz_config::theme::Theme;
use yinz_types::{data::PaletteColor, input::cli_assets::CliAssets};

fn theme(base: u8) -> Theme {
    let mut theme = Theme {
        palette: Default::default(),
        terminal: None,
        sourced_from_external_file: false,
    };
    theme.palette.text_unselected.base = PaletteColor::EightBit(base);
    theme
}

fn base(config: &Config, name: &str) -> Option<PaletteColor> {
    config
        .themes
        .get_theme(name)
        .map(|theme| theme.palette.text_unselected.base)
}

/// A replacement server whose startup assets target `operation` and read themes from `config_dir`.
fn replacement(
    operation: &str,
    config_dir: &std::path::Path,
) -> (
    Arc<RwLock<SessionState>>,
    Arc<RwLock<Option<SessionMetaData>>>,
) {
    let mut state = SessionState::new();
    state.update_startup_assets = Some(CliAssets {
        config_dir: Some(config_dir.into()),
        update_target: Some(operation.into()),
        ..Default::default()
    });
    (
        Arc::new(RwLock::new(state)),
        Arc::new(RwLock::new(Some(
            crate::session_shutdown_tests::empty_session(),
        ))),
    )
}

fn restore(operation: &str, clients: Vec<session_update::Client>) -> session_update::Request {
    session_update::Request {
        token: "token".into(),
        operation: operation.into(),
        command: session_update::Command::RestoreClients { clients },
    }
}

/// Writes a theme directory under `config_dir` holding `name` with `base`.
fn write_theme_dir(config_dir: &std::path::Path, name: &str, base: u8) {
    let themes_dir = config_dir.join("themes");
    std::fs::create_dir_all(&themes_dir).unwrap();
    let mut file_themes = Config::default();
    file_themes.themes.insert(name.into(), theme(base));
    std::fs::write(
        themes_dir.join("mine.kdl"),
        file_themes.to_string(false).unwrap(),
    )
    .unwrap();
}

/// Restores one client with `configuration` and carried `themes` on a replacement reading
/// themes from `config_dir`, returning the recorded configuration.
fn restore_one(
    config_dir: &std::path::Path,
    configuration: &Config,
    themes: Option<String>,
) -> Config {
    let configuration = configuration.to_string(false).unwrap();
    assert_eq!(
        Config::from_kdl(&configuration, None)
            .unwrap()
            .themes
            .inner()
            .values()
            .filter(|theme| theme.sourced_from_external_file)
            .count(),
        0,
        "the configuration KDL must not already carry file themes"
    );
    let operation = uuid::Uuid::new_v4().to_string();
    let (state, data) = replacement(&operation, config_dir);
    let client = ClientId::from(7);
    let reply = handle(
        restore(
            &operation,
            vec![session_update::Client {
                id: client,
                supports_update: true,
                supports_source_update: false,
                resumed: None,
                configuration,
                themes,
                mode: InputMode::Locked,
            }],
        ),
        1.into(),
        &state,
        &data,
    );
    assert_eq!(reply.result, Ok(serde_json::Value::Null));

    let state = state.read().unwrap();
    let (restored, mode) = state
        .pending_update_clients
        .get(&(operation, client))
        .expect("client restoration is recorded");
    assert_eq!(*mode, InputMode::Locked);
    restored.clone()
}

/// The source server sends each client's configuration as KDL, which omits file-sourced themes,
/// and sends those themes alongside. Restoration uses what the source held, not whatever the
/// theme directory holds by now.
#[test]
fn restored_clients_keep_the_file_themes_their_source_held() {
    let dir = tempfile::tempdir().unwrap();
    write_theme_dir(dir.path(), "from-dir", 9);

    let mut source = Config::default();
    source.themes.insert("inline".into(), theme(2));
    for (name, base) in [("from-dir", 1), ("deleted-since", 3)] {
        let mut file_theme = theme(base);
        file_theme.sourced_from_external_file = true;
        source.themes.insert(name.into(), file_theme);
    }
    let carried = yinz_config::cli_assets::file_themes_to_string(&source);

    let restored = restore_one(dir.path(), &source, Some(carried));
    // The replacement adds its own built-in themes beneath these.
    for (name, theme) in source.themes.inner() {
        assert_eq!(restored.themes.get_theme(name), Some(theme), "{name}");
    }
    assert_eq!(base(&restored, "from-dir"), Some(PaletteColor::EightBit(1)));
    assert_eq!(
        base(&restored, "deleted-since"),
        Some(PaletteColor::EightBit(3))
    );
}

/// Sources that predate carried themes send none; the replacement rereads the theme directory.
#[test]
fn restored_clients_from_older_sources_reread_the_theme_directory() {
    let dir = tempfile::tempdir().unwrap();
    write_theme_dir(dir.path(), "from-dir", 1);
    let mut source = Config::default();
    source.themes.insert("inline".into(), theme(2));

    let restored = restore_one(dir.path(), &source, None);
    assert_eq!(base(&restored, "from-dir"), Some(PaletteColor::EightBit(1)));
    assert_eq!(base(&restored, "inline"), Some(PaletteColor::EightBit(2)));
}

#[test]
fn restoration_for_another_update_is_refused_before_recording_clients() {
    let dir = tempfile::tempdir().unwrap();
    let (state, data) = replacement(&uuid::Uuid::new_v4().to_string(), dir.path());
    let other = uuid::Uuid::new_v4().to_string();
    let reply = handle(
        restore(
            &other,
            vec![session_update::Client {
                id: 7.into(),
                supports_update: true,
                supports_source_update: false,
                resumed: None,
                configuration: Config::default().to_string(false).unwrap(),
                themes: Some(String::new()),
                mode: InputMode::Normal,
            }],
        ),
        1.into(),
        &state,
        &data,
    );
    assert!(
        reply
            .result
            .as_ref()
            .is_err_and(|error| error.contains("different update target")),
        "{:?}",
        reply.result
    );
    assert!(state.read().unwrap().pending_update_clients.is_empty());
}
