use super::*;
use std::process::{Command, Stdio};

#[test]
fn installed_claude_hooks_quote_the_binary_path_for_local_and_remote_reports() {
    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp
        .path()
        .join("user's tools $(touch expanded) `touch backtick-expanded` $EXPANSION");
    std::fs::create_dir(&bin_dir).unwrap();
    let reporter = bin_dir.join("yinz");
    let argv = temp.path().join("argv");
    let input = temp.path().join("input");
    let captured = temp.path().join("captured");
    yinz_test_support::write_warm_script(
        &reporter,
        "printf '%s\\0' \"$@\" > \"$ARGV\"\n/bin/cat > \"$CAPTURE\"",
    );
    let installed = temp.path().join("yinz-hook.sh");
    std::fs::write(&installed, script(HookAgent::Claude, &reporter)).unwrap();
    let payload = b"{\"hook_event_name\":\"SessionEnd\",\"session_id\":\"user's $(touch payload-expanded) $EXPANSION\"}\n";
    std::fs::write(&input, payload).unwrap();
    for remote in [false, true] {
        std::fs::write(&argv, "").unwrap();
        std::fs::write(&captured, "").unwrap();
        let output = Command::new("/bin/sh")
            .arg(&installed)
            .current_dir(temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("EXPANSION", "expanded-variable")
            .env("ARGV", &argv)
            .env("CAPTURE", &captured)
            .env("YINZ_PANE_REF", "abcdef012345")
            .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
            .env(
                yinz_server::pty_host::remote::broker::PANE_TOKEN_ENV,
                if remote { "token" } else { "" },
            )
            .stdin(Stdio::from(std::fs::File::open(&input).unwrap()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(
            std::fs::read(&argv).unwrap(),
            if remote {
                b"--pty-broker\0report\0claude\0".as_slice()
            } else {
                b"action\0agent-report\0--hook-json\0".as_slice()
            }
        );
        assert_eq!(std::fs::read(&captured).unwrap(), payload);
        for expanded in ["expanded", "backtick-expanded", "payload-expanded"] {
            assert!(!temp.path().join(expanded).exists());
        }
    }
}

#[test]
fn installed_and_inline_claude_hooks_report_once_with_unchanged_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let reporter = temp.path().join("yinz");
    let reports = temp.path().join("reports");
    yinz_test_support::write_warm_script(
        &reporter,
        "printf '%s\\n' \"$*\" >> \"$REPORTS\"\n/bin/cat >> \"$REPORTS\"",
    );
    let installed = temp.path().join("yinz-hook.sh");
    std::fs::write(&installed, script(HookAgent::Claude, &reporter)).unwrap();
    let settings: Value =
        serde_json::from_str(&yinz_runtime::agent::launch::claude_hook_settings_argument())
            .unwrap();
    let empty_path = temp.path().join("empty");
    std::fs::create_dir(&empty_path).unwrap();
    for path in [temp.path(), empty_path.as_path()] {
        for marker in [None, Some("0"), Some("1")] {
            for remote in [false, true] {
                for event in CLAUDE_EVENTS {
                    std::fs::write(&reports, "").unwrap();
                    let payload = format!("{{\"hook_event_name\":\"{event}\",\"session_id\":\"conversation\",\"last_assistant_message\":\"done\"}}\n");
                    let input = temp.path().join("input");
                    std::fs::write(&input, &payload).unwrap();
                    let mut commands = vec![vec![installed.to_str().unwrap()]];
                    if marker == Some("1") {
                        commands.push(vec![
                            "-c",
                            settings["hooks"][*event][0]["hooks"][0]["command"]
                                .as_str()
                                .unwrap(),
                        ]);
                    }
                    for args in commands {
                        let mut command = Command::new("/bin/sh");
                        command
                            .args(args)
                            .env("PATH", path)
                            .env("REPORTS", &reports)
                            .env("YINZ_PANE_REF", "abcdef012345")
                            .env_remove("YINZ_CLAUDE_INLINE_HOOKS")
                            .env("YINZ_REMOTE_PANE_TOKEN", if remote { "token" } else { "" })
                            .stdin(Stdio::from(std::fs::File::open(&input).unwrap()));
                        if let Some(marker) = marker {
                            command.env("YINZ_CLAUDE_INLINE_HOOKS", marker);
                        }
                        let output = command.output().unwrap();
                        assert!(output.status.success(), "{output:?}");
                        assert!(output.stdout.is_empty());
                        assert!(output.stderr.is_empty(), "{output:?}");
                    }
                    let route = if remote {
                        "--pty-broker report claude"
                    } else {
                        "action agent-report --hook-json"
                    };
                    assert_eq!(
                        std::fs::read_to_string(&reports).unwrap(),
                        format!("{route}\n{payload}")
                    );
                }
            }
        }
    }
}
