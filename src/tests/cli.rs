use std::net::{IpAddr, Ipv4Addr};

use clap::{CommandFactory, Parser};
use zellij_utils::cli::{CliArgs, Command};

const CLI_PARSE_STACK_SIZE: usize = 32 * 1024 * 1024;

/// clap's derived builder for `CliArgs` needs megabytes of stack in a debug build (the
/// derive expands the huge `CliAction` enum into one unoptimized function), close to
/// libtest's 2MiB per-test threads. Parse on a thread with room.
fn try_parse_cli<I, S>(args: I) -> Result<CliArgs, clap::Error>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    std::thread::Builder::new()
        .stack_size(CLI_PARSE_STACK_SIZE)
        .spawn(move || CliArgs::try_parse_from(args))
        .expect("failed to spawn cli parse thread")
        .join()
        .expect("cli parse thread panicked")
}

#[test]
fn verify_cli() {
    std::thread::Builder::new()
        .stack_size(CLI_PARSE_STACK_SIZE)
        .spawn(|| CliArgs::command().debug_assert())
        .expect("failed to spawn cli parse thread")
        .join()
        .expect("cli parse thread panicked");
}

#[test]
fn web_cli_status_alone_works() {
    let args = try_parse_cli(["zellij", "web", "--status"]);
    assert!(args.is_ok());
    if let Ok(CliArgs {
        command: Some(Command::Web(web)),
        ..
    }) = args
    {
        assert!(web.status);
        assert!(web.timeout.is_none());
    } else {
        panic!("Expected Web command");
    }
}

#[test]
fn web_cli_status_with_timeout_works() {
    let args = try_parse_cli(["zellij", "web", "--status", "--timeout", "5"]);
    assert!(args.is_ok());
    if let Ok(CliArgs {
        command: Some(Command::Web(web)),
        ..
    }) = args
    {
        assert!(web.status);
        assert_eq!(web.timeout, Some(5));
    } else {
        panic!("Expected Web command");
    }
}

#[test]
fn web_cli_timeout_with_status_works() {
    // Test with --timeout before --status (order shouldn't matter)
    let args = try_parse_cli(["zellij", "web", "--timeout", "10", "--status"]);
    assert!(args.is_ok());
    if let Ok(CliArgs {
        command: Some(Command::Web(web)),
        ..
    }) = args
    {
        assert!(web.status);
        assert_eq!(web.timeout, Some(10));
    } else {
        panic!("Expected Web command");
    }
}

#[test]
fn web_cli_timeout_without_status_fails() {
    let args = try_parse_cli(["zellij", "web", "--timeout", "5"]);
    assert!(args.is_err());
}

#[test]
fn web_cli_status_with_start_fails() {
    let args = try_parse_cli(["zellij", "web", "--status", "--start"]);
    assert!(args.is_err());
}

#[test]
fn web_cli_status_with_stop_fails() {
    let args = try_parse_cli(["zellij", "web", "--status", "--stop"]);
    assert!(args.is_err());
}

#[test]
fn web_cli_status_with_ip_works() {
    let args = try_parse_cli(["zellij", "web", "--status", "--ip", "127.0.0.1"]);
    assert!(args.is_ok());
    if let Ok(CliArgs {
        command: Some(Command::Web(web)),
        ..
    }) = args
    {
        assert!(web.status);
        assert_eq!(web.ip, Some(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    } else {
        panic!("Expected Web command");
    }
}

#[test]
fn web_cli_status_with_port_works() {
    let args = try_parse_cli(["zellij", "web", "--status", "--port", "9000"]);
    assert!(args.is_ok());
    if let Ok(CliArgs {
        command: Some(Command::Web(web)),
        ..
    }) = args
    {
        assert!(web.status);
        assert_eq!(web.port, Some(9000));
    } else {
        panic!("Expected Web command");
    }
}

#[test]
fn web_cli_status_with_ip_and_port_works() {
    let args = try_parse_cli([
        "zellij", "web", "--status", "--ip", "0.0.0.0", "--port", "9000",
    ]);
    assert!(args.is_ok());
    if let Ok(CliArgs {
        command: Some(Command::Web(web)),
        ..
    }) = args
    {
        assert!(web.status);
        assert_eq!(web.ip, Some(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
        assert_eq!(web.port, Some(9000));
    } else {
        panic!("Expected Web command");
    }
}

#[test]
fn cli_recordings_list_parses() {
    let args = try_parse_cli(["zellij", "recordings", "list"]).unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::List { json: false })) => {},
        _ => panic!("Expected Recordings::List"),
    }
}

#[test]
fn cli_recordings_list_json_parses() {
    let args = try_parse_cli(["zellij", "recordings", "list", "--json"]).unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::List { json: true })) => {},
        _ => panic!("Expected Recordings::List with json"),
    }
}

#[test]
fn cli_recordings_export_parses() {
    let args = try_parse_cli(["zellij", "recordings", "export", "42"]).unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::Export {
            recording_id: 42,
            output: None,
        })) => {},
        _ => panic!("Expected Recordings::Export without output"),
    }
}

#[test]
fn cli_recordings_unknown_subcommand_fails() {
    let res = try_parse_cli(["zellij", "recordings", "bogus"]);
    assert!(res.is_err(), "unknown subcommand should fail");
}

#[test]
fn cli_recordings_export_missing_id_fails() {
    let res = try_parse_cli(["zellij", "recordings", "export"]);
    assert!(res.is_err(), "missing recording_id should fail");
}

#[test]
fn cli_recordings_export_non_numeric_id_fails() {
    let res = try_parse_cli(["zellij", "recordings", "export", "abc"]);
    assert!(res.is_err(), "non-numeric id should fail");
}

#[test]
fn cli_recordings_export_with_output_parses() {
    use std::path::PathBuf;
    let args = try_parse_cli([
        "zellij",
        "recordings",
        "export",
        "42",
        "--output",
        "pane.cast",
    ])
    .unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::Export {
            recording_id: 42,
            output: Some(path),
        })) if path == PathBuf::from("pane.cast") => {},
        _ => panic!("Expected Recordings::Export with output"),
    }
}

#[test]
fn cli_recordings_search_parses() {
    let args = try_parse_cli(["zellij", "recordings", "search", "needle"]).unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::Search { query, limit, json })) => {
            assert_eq!(query, "needle");
            assert_eq!(limit, 50, "default limit");
            assert!(!json, "default is the human-readable table");
        },
        _ => panic!("Expected Recordings::Search"),
    }
}

#[test]
fn cli_recordings_search_limit_parses() {
    let args = try_parse_cli(["zellij", "recordings", "search", "needle", "--limit", "5"]).unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::Search { limit: 5, .. })) => {},
        _ => panic!("Expected Recordings::Search with limit 5"),
    }
}

#[test]
fn cli_recordings_search_missing_query_fails() {
    let res = try_parse_cli(["zellij", "recordings", "search"]);
    assert!(res.is_err(), "missing query should fail");
}

#[test]
fn recordings_search_output_formats_header_and_rows() {
    use zellij_utils::session_recording::SearchHit;

    assert_eq!(crate::commands::format_search_results(&[]), "No matches.\n");

    let hits = vec![SearchHit {
        recording_id: 3,
        session_name: "my_session".to_string(),
        tab_name: "my_tab".to_string(),
        pane_title: String::new(),
        elapsed_us: 2_500_000,
        started_at_us: 1_000_000,
        text: "hello needle world".to_string(),
    }];
    let output = crate::commands::format_search_results(&hits);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "expected one header row and one data row");
    for column in ["ID", "TIME", "SESSION", "TAB", "PANE", "TEXT"] {
        assert!(lines[0].contains(column), "header must contain {}", column);
    }
    assert!(lines[1].contains("my_session"));
    assert!(lines[1].contains("my_tab"));
    assert!(lines[1].contains("hello needle world"));
    assert!(
        lines[1].contains("2.5s"),
        "expected offset, got: {}",
        lines[1]
    );
}

#[test]
fn recordings_list_output_formats_header_and_rows() {
    use std::fs;
    use zellij_utils::session_recording::{PaneMetadata, RecordingStore};

    // create temp recording database with one pane (unique per test invocation to avoid pid collision)
    let dir_path = std::env::temp_dir().join(format!(
        "zellij-recordings-test-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let _ = fs::remove_dir_all(&dir_path);
    fs::create_dir_all(&dir_path).unwrap();
    let mut store = RecordingStore::open(&dir_path, "my_session", true, 1_000_000).unwrap();
    let meta = PaneMetadata {
        terminal_id: 7,
        tab_id: 0,
        tab_position: 0,
        tab_name: "my_tab".to_string(),
        title: "my_pane".to_string(),
        cols: 80,
        rows: 24,
    };
    store.update_panes(&[meta], 0).unwrap();
    let summaries = zellij_utils::session_recording::list_recordings(&dir_path).unwrap();
    assert_eq!(summaries.len(), 1);
    // call formatting helper (does not exist yet, should fail)
    let output = crate::commands::format_recordings_list(&summaries);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 2, "expected one header row and one data row");
    assert!(lines[0].contains("ID"), "header must contain ID");
    assert!(lines[0].contains("SESSION"), "header must contain SESSION");
    assert!(lines[0].contains("TAB"), "header must contain TAB");
    assert!(lines[0].contains("PANE"), "header must contain PANE");
    assert!(lines[0].contains("STARTED"), "header must contain STARTED");
    assert!(
        lines[0].contains("DURATION"),
        "header must contain DURATION"
    );
    assert!(lines[0].contains("INPUT"), "header must contain INPUT");
    assert!(lines[0].contains("STATE"), "header must contain STATE");
    // data row checks
    assert!(lines[1].contains("my_session"));
    assert!(lines[1].contains("my_tab"));
    // running state for open pane (is_running=true, so should be running)
    assert!(
        lines[1].contains("running"),
        "expected running for open pane, got: {}",
        lines[1]
    );
    assert!(
        lines[1].contains("yes"),
        "expected yes for capture_input true, got: {}",
        lines[1]
    );
    let _ = fs::remove_dir_all(&dir_path);
}

#[test]
fn recordings_state_strings() {
    use zellij_utils::session_recording::RecordingSummary;

    let running = RecordingSummary {
        id: 1,
        session_name: "s".to_string(),
        tab_name: "t".to_string(),
        terminal_id: 1,
        pane_title: "p".to_string(),
        started_at_us: 0,
        duration_us: 0,
        is_running: true,
        capture_input: true,
        error: None,
    };
    let complete = RecordingSummary {
        id: 2,
        session_name: "s".to_string(),
        tab_name: "t".to_string(),
        terminal_id: 2,
        pane_title: "p".to_string(),
        started_at_us: 0,
        duration_us: 5_000_000,
        is_running: false,
        capture_input: false,
        error: None,
    };
    let incomplete = RecordingSummary {
        id: 3,
        session_name: "s".to_string(),
        tab_name: "t".to_string(),
        terminal_id: 3,
        pane_title: "p".to_string(),
        started_at_us: 0,
        duration_us: 0,
        is_running: false,
        capture_input: false,
        error: Some("oops".to_string()),
    };
    let output = crate::commands::format_recordings_list(&[running, complete, incomplete]);
    let lines: Vec<&str> = output.lines().collect();
    assert_eq!(lines.len(), 4, "header + 3 rows");
    assert!(
        lines[1].contains("running"),
        "running pane should show running"
    );
    assert!(
        lines[2].contains("complete"),
        "closed pane without error should show complete"
    );
    assert!(
        lines[3].contains("incomplete"),
        "pane with error should show incomplete"
    );
}
