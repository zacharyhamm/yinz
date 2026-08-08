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
fn cli_recordings_export_with_output_parses() {
    use std::path::PathBuf;
    let args =
        CliArgs::try_parse_from(["zellij", "recordings", "export", "42", "--output", "pane.cast"])
            .unwrap();
    match args.command {
        Some(Command::Recordings(zellij_utils::cli::Recordings::Export {
            recording_id: 42,
            output: Some(path),
        })) if path == PathBuf::from("pane.cast") => {},
        _ => panic!("Expected Recordings::Export with output"),
    }
}
