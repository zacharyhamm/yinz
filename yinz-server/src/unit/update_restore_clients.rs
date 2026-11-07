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

/// The source server sends each client's configuration as KDL, which omits themes loaded from the
/// theme directory. Restoration must bring them back so a client's selected file theme survives.
#[test]
fn restored_clients_regain_theme_directory_themes() {
    let dir = tempfile::tempdir().unwrap();
    let themes_dir = dir.path().join("themes");
    std::fs::create_dir(&themes_dir).unwrap();
    let mut file_themes = Config::default();
    file_themes.themes.insert("from-dir".into(), theme(1));
    std::fs::write(
        themes_dir.join("mine.kdl"),
        file_themes.to_string(false).unwrap(),
    )
    .unwrap();

    let mut source = Config::default();
    source.themes.insert("inline".into(), theme(2));
    let configuration = source.to_string(false).unwrap();
    assert_eq!(
        base(&Config::from_kdl(&configuration, None).unwrap(), "from-dir"),
        None,
        "the source configuration must not already carry the directory theme"
    );

    let operation = uuid::Uuid::new_v4().to_string();
    let (state, data) = replacement(&operation, dir.path());
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
    assert_eq!(base(restored, "from-dir"), Some(PaletteColor::EightBit(1)));
    assert_eq!(base(restored, "inline"), Some(PaletteColor::EightBit(2)));
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
