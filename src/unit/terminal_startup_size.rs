//! Real commands read their assigned geometry before doing any application work.
use super::*;
use nix::libc;

#[test]
fn first_winsize_child() {
    if std::env::var_os("YINZ_FIRST_WINSIZE_CHILD").is_none() {
        return;
    }
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) }, 0);
    println!(
        "W={},{},{},{}",
        size.ws_col, size.ws_row, size.ws_xpixel, size.ws_ypixel
    );
}

fn child_args() -> Vec<String> {
    vec![
        "/usr/bin/env".into(),
        "YINZ_FIRST_WINSIZE_CHILD=1".into(),
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        "--exact".into(),
        "terminal_startup_size::first_winsize_child".into(),
        "--nocapture".into(),
    ]
}

fn panes(session: &Session) -> Vec<Value> {
    success(session.run(&["action", "list-panes", "--all", "--json"], None))
        .as_array()
        .unwrap()
        .clone()
}

fn assert_first_size(session: &Session, pane: &Value) {
    let id = format!("terminal_{}", pane["id"].as_u64().unwrap());
    success(session.run(&["action", "pane-wait", &id, "--exited"], None));
    let expected = format!(
        "W={},{},0,0",
        pane["pane_content_columns"], pane["pane_content_rows"]
    );
    success(session.run(&["action", "pane-wait", &id, "--contains", &expected], None));
    let dump = session.run(&["action", "dump-screen", "--pane-id", &id, "--full"], None);
    assert!(dump.status.success());
    let output = String::from_utf8(dump.stdout).unwrap();
    assert!(
        output.contains(&expected),
        "{id}: expected {expected}; got {output}"
    );
}

#[test]
fn six_layout_commands_start_with_their_content_size_in_new_and_overridden_tabs() {
    for detachable in [false, true] {
        let session = Session::start(
            |dir| {
                if detachable {
                    let path = dir.join("config.kdl");
                    std::fs::write(
                        &path,
                        std::fs::read_to_string(&path)
                            .unwrap()
                            .replace("detachable_panes false", "detachable_panes true"),
                    )
                    .unwrap();
                }
            },
            false,
        );
        let args = child_args();
        let slot = format!(
            "pane command={} {{ args {}; }}",
            serde_json::to_string(&args[0]).unwrap(),
            args[1..]
                .iter()
                .map(|arg| serde_json::to_string(arg).unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let row = format!("pane split_direction=\"vertical\" {{ {slot}; {slot}; }}");
        let layout = format!("layout {{ {row}; {row}; {row}; }}");
        for action in ["new-tab", "override-layout"] {
            let out = session.run(&["action", action, "--layout-string", &layout], None);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let children: Vec<_> = panes(&session)
                .into_iter()
                .filter(|pane| {
                    pane["terminal_command"]
                        .as_str()
                        .is_some_and(|cmd| cmd.contains("YINZ_FIRST_WINSIZE_CHILD"))
                })
                .collect();
            assert_eq!(children.len(), 6);
            for pane in children {
                assert_first_size(&session, &pane);
            }
        }
    }
}

#[test]
fn single_floating_in_place_held_and_resized_reruns_start_with_current_geometry() {
    let session = Session::new();
    for options in [
        vec!["--direction", "right"],
        vec!["--floating", "--width", "50", "--height", "12"],
        vec!["--in-place"],
        vec!["--start-suspended"],
    ] {
        let args = child_args();
        let mut action = vec!["action", "new-pane"];
        action.extend_from_slice(&options);
        if options.contains(&"--in-place") {
            action.extend_from_slice(&["--pane-id", &session.1]);
        }
        action.push("--");
        action.extend(args.iter().map(String::as_str));
        let out = session.run(&action, None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let id = String::from_utf8(out.stdout).unwrap().trim().to_owned();
        if options.contains(&"--start-suspended") {
            let dump = session.run(&["action", "dump-screen", "--pane-id", &id], None);
            assert!(!String::from_utf8_lossy(&dump.stdout).contains("W="));
            success(session.run(&["action", "pane-rerun", &id], None));
        }
        let pane = panes(&session)
            .into_iter()
            .find(|pane| format!("terminal_{}", pane["id"]) == id)
            .unwrap();
        assert_first_size(&session, &pane);
        if options.contains(&"--floating") {
            let resize = session.run(
                &[
                    "action",
                    "change-floating-pane-coordinates",
                    "--pane-id",
                    &id,
                    "--width",
                    "60",
                    "--height",
                    "15",
                ],
                None,
            );
            assert!(
                resize.status.success(),
                "{}",
                String::from_utf8_lossy(&resize.stderr)
            );
            success(session.run(&["action", "pane-rerun", &id], None));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let resized = loop {
                let updated = panes(&session)
                    .into_iter()
                    .find(|candidate| candidate["id"] == pane["id"])
                    .unwrap();
                if (
                    updated["pane_content_columns"].clone(),
                    updated["pane_content_rows"].clone(),
                ) != (
                    pane["pane_content_columns"].clone(),
                    pane["pane_content_rows"].clone(),
                ) {
                    break updated;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "floating resize was not reported: {updated}"
                );
            };
            assert_first_size(&session, &resized);
        }
        let out = session.run(&["action", "close-pane", "--pane-id", &id], None);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
