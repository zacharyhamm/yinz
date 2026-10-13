use super::*;
use std::io::{BufRead, Write};
use yinz_test_support::{default_timeout, write_warm_script};
use yinz_types::data::CopyDestination;
use yinz_types::internal_ids::PluginId;

fn clipboard_events(instructions: &[PluginInstruction]) -> Vec<Event> {
    instructions
        .iter()
        .filter_map(|instruction| match instruction {
            PluginInstruction::Update(events) => Some(events),
            _ => None,
        })
        .flatten()
        .filter_map(|(_, _, event)| match event {
            Event::CopyToClipboard(_) | Event::SystemClipboardFailure => Some(event.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn stalled_clipboard_helper_does_not_block_screen_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let ready_path = dir.path().join("ready");
    let release_path = dir.path().join("release");
    for path in [&ready_path, &release_path] {
        assert!(std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap()
            .success());
    }
    let mut ready_pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&ready_path)
        .unwrap();
    let mut release_pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&release_path)
        .unwrap();
    let script = dir.path().join("helper");
    let received = dir.path().join("received");
    write_warm_script(
        &script,
        &format!(
            "echo ready > '{}'\nread release < '{}'\ncat > '{}'\necho done > '{}'",
            ready_path.display(),
            release_path.display(),
            received.display(),
            ready_path.display()
        ),
    );
    let size = Size { cols: 80, rows: 24 };
    let mut mock_screen = MockScreen::new(size);
    mock_screen.config.options.copy_command = Some(script.display().to_string());
    let layout = TiledPaneLayout {
        children: vec![
            TiledPaneLayout::default(),
            TiledPaneLayout {
                run: Some(Run::Plugin(RunPluginOrAlias::RunPlugin(RunPlugin {
                    location: RunPluginLocation::File(PathBuf::from("/path/to/fake/plugin")),
                    ..Default::default()
                }))),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let screen_thread = mock_screen.run(Some(layout), vec![]);
    let server_log = Watched::new(vec![]);
    let server_receiver = mock_screen.server_receiver.take().unwrap();
    let server_thread =
        log_actions_in_thread!(server_log, ServerInstruction::KillSession, server_receiver);
    let plugin_log = Watched::new(vec![]);
    let plugin_receiver = mock_screen.plugin_receiver.take().unwrap();
    let plugin_thread =
        log_actions_in_thread!(plugin_log, PluginInstruction::Exit, plugin_receiver);
    let ready = Watched::new(0);
    let ready_reader = ready_pipe.try_clone().unwrap();
    let ready_thread = std::thread::spawn({
        let ready = ready.clone();
        move || {
            let mut reader = std::io::BufReader::new(ready_reader);
            for count in 1..=2 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                *ready.lock() = count;
            }
        }
    });
    let text = "x".repeat(4 * 1024 * 1024);
    // Always release the helper before tearing down, including when the old blocking path fails.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mock_screen
            .to_screen
            .send(ScreenInstruction::CopyTextToClipboard(
                text.clone(),
                PluginId::new(1),
            ))
            .unwrap();
        ready.wait_until("clipboard helper ready", default_timeout(), |ready| {
            (*ready >= 1).then_some(())
        });
        mock_screen
            .to_screen
            .send(ScreenInstruction::PtyBytes(
                TerminalId::new(0),
                b"screen still responsive".to_vec(),
            ))
            .unwrap();
        wait_for_settled_render(
            &server_log,
            size,
            "render while clipboard stdin is blocked",
            |frame| frame.contains("screen still responsive"),
        );
        assert!(clipboard_events(&plugin_log.lock()).is_empty());
    }));
    writeln!(release_pipe).unwrap();
    let completion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ready.wait_until(
            "clipboard helper consumed input",
            default_timeout(),
            |ready| (*ready == 2).then_some(()),
        );
        plugin_log.wait_until(
            "clipboard transfer completion",
            default_timeout(),
            |events| (!clipboard_events(events).is_empty()).then_some(()),
        );
    }));
    writeln!(ready_pipe, "cleanup\ncleanup").unwrap();
    ready_thread.join().unwrap();
    mock_screen.teardown(vec![screen_thread, server_thread, plugin_thread]);
    if let Err(panic) = result.and(completion) {
        std::panic::resume_unwind(panic);
    }
    assert_eq!(
        clipboard_events(&plugin_log.lock()),
        vec![Event::CopyToClipboard(CopyDestination::Command)]
    );
    let contents = std::fs::read_to_string(&received).unwrap();
    assert_eq!(contents, text);
}
