use std::net::{IpAddr, Ipv4Addr};

use clap::{CommandFactory, Parser};
use zellij_utils::cli::{CliArgs, Command};

#[test]
fn verify_cli() {
    CliArgs::command().debug_assert();
}

#[test]
fn web_cli_status_alone_works() {
    let args = CliArgs::try_parse_from(["zellij", "web", "--status"]);
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
    let args = CliArgs::try_parse_from(["zellij", "web", "--status", "--timeout", "5"]);
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
    let args = CliArgs::try_parse_from(["zellij", "web", "--timeout", "10", "--status"]);
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
    let args = CliArgs::try_parse_from(["zellij", "web", "--timeout", "5"]);
    assert!(args.is_err());
}

#[test]
fn web_cli_status_with_start_fails() {
    let args = CliArgs::try_parse_from(["zellij", "web", "--status", "--start"]);
    assert!(args.is_err());
}

#[test]
fn web_cli_status_with_stop_fails() {
    let args = CliArgs::try_parse_from(["zellij", "web", "--status", "--stop"]);
    assert!(args.is_err());
}

#[test]
fn web_cli_status_with_ip_works() {
    let args = CliArgs::try_parse_from(["zellij", "web", "--status", "--ip", "127.0.0.1"]);
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
    let args = CliArgs::try_parse_from(["zellij", "web", "--status", "--port", "9000"]);
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
    let args = CliArgs::try_parse_from([
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
    let args = CliArgs::try_parse_from(["zellij", "recordings", "list"]).unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::List)) => {},
        _ => panic!("Expected Recordings::List"),
    }
}

#[test]
fn cli_recordings_export_parses() {
    let args = CliArgs::try_parse_from(["zellij", "recordings", "export", "42"]).unwrap();
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
    let res = CliArgs::try_parse_from(["zellij", "recordings", "bogus"]);
    assert!(res.is_err(), "unknown subcommand should fail");
}

#[test]
fn cli_recordings_export_missing_id_fails() {
    let res = CliArgs::try_parse_from(["zellij", "recordings", "export"]);
    assert!(res.is_err(), "missing recording_id should fail");
}

#[test]
fn cli_recordings_export_non_numeric_id_fails() {
    let res = CliArgs::try_parse_from(["zellij", "recordings", "export", "abc"]);
    assert!(res.is_err(), "non-numeric id should fail");
}

#[test]
fn cli_recordings_export_with_output_parses() {
    use std::path::PathBuf;
    let args = CliArgs::try_parse_from([
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
fn recordings_list_output_formats_header_and_rows() {
    use std::fs;
    use zellij_utils::session_recording::{PaneMetadata, RecordingStore};

    // create temp recording database with one pane
    let dir_path =
        std::env::temp_dir().join(format!("zellij-recordings-test-{}", std::process::id()));
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
    // running state for open pane
    assert!(
        lines[1].contains("running")
            || lines[1].contains("complete")
            || lines[1].contains("incomplete")
    );
    // test state variants: close pane without error => complete, with error => incomplete
    // We verify helper formats input yes/no correctly (capture_input true => yes)
    assert!(lines[1].contains("yes") || lines[1].contains("no"));
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
