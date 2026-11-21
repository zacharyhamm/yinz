//! Display-free graphical client driven by the real-binary switch fixture.
use super::*;

/// Drive the actual window key, reader, staging and promotion paths. Holding the
/// destination's wake events lets source input deterministically overtake its repaint.
pub fn exercise_live_switch(opts: CliArgs, cli_switch: impl FnOnce()) -> Result<()> {
    let args = TerminalArgs {
        session_name: Some("source".into()),
        rows: Some(24),
        cols: Some(80),
        ..Default::default()
    };
    let (wake, wakes) = mpsc::channel();
    let config = load_native_config(&opts)?;
    let mut app = App::new(
        args,
        opts,
        config,
        Arc::new(move |id| wake.send(id).is_ok()),
        Clipboard::in_memory,
    )?;
    app.open_connection(None)?;
    let until =
        |app: &mut App, what: &str, promote: bool, done: &dyn Fn(&App) -> bool| -> Result<()> {
            let limit = Instant::now() + Duration::from_secs(15);
            while !done(app) {
                let id = wakes
                    .recv_timeout(limit.saturating_duration_since(Instant::now()))
                    .with_context(|| {
                        format!("waiting for {what} in {:?}", app.args.session_name)
                    })?;
                if id == app.connection_id {
                    if let Some(message) = receive_message(app.messages.as_ref())? {
                        app.handle_message(message, || panic!("source exited during {what}"))?;
                    }
                } else if promote {
                    app.receive_switch(id, Instant::now())?;
                }
            }
            Ok(())
        };
    let content = |app: &App, marker: &str| {
        let size = app.state.size();
        (0..size.rows).any(|row| {
            (0..size.cols)
                .map(|col| app.state.screen().cell(row, col).character())
                .collect::<String>()
                .contains(marker)
        })
    };
    let key = |app: &mut App, ch: &str, mods| {
        app.key_pressed(
            &keyboard::Press::new(Key::Character(ch.into()), mods),
            false,
        )
    };
    let probe = |app: &mut App, ch: &str, marker: &str| -> Result<()> {
        key(app, ch, ModifiersState::empty())?;
        until(app, marker, false, &|app| content(app, marker))
    };
    let request = |app: &mut App| -> Result<()> {
        key(app, "y", ModifiersState::CONTROL | ModifiersState::ALT)?;
        until(app, "server switch", false, &|app| {
            app.pending_switch.is_some()
        })
    };
    let escape = keyboard::Press::new(Key::Named(NamedKey::Escape), ModifiersState::empty());
    until(&mut app, "source opening", false, &|app| {
        content(app, "source-ready")
    })?;
    cli_switch();
    until(&mut app, "CLI switch", false, &|app| {
        app.pending_switch.is_some()
    })?;
    app.key_pressed(&escape, false)?;
    probe(&mut app, "a", "source-a")?;
    request(&mut app)?;
    probe(&mut app, "b", "source-b")?;
    app.key_pressed(&escape, false)?;
    probe(&mut app, "c", "source-c")?;
    request(&mut app)?;
    let failed = app.pending_switch.as_ref().context("pending destination")?;
    app.receive_switch(failed.id, failed.deadline)?;
    anyhow::ensure!(app.switch_notice.is_some(), "missing switch timeout notice");
    probe(&mut app, "d", "source-d")?;
    request(&mut app)?;
    until(&mut app, "destination repaint", true, &|app| {
        app.args.session_name.as_deref() == Some("destination")
    })?;
    probe(&mut app, "e", "destination-e")
}
