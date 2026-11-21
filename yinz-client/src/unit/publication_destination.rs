//! The receiving half of the server's ANSI publication regression.
use super::*;
use crate::os_input_output::{get_cli_client_os_input, ServerConnection};
use crate::test_support::FakeClientOsApi;
use yinz_ipc::ipc::ServerToClientMsg;
use yinz_key_codec::vendored::termwiz::input::InputEvent;
use yinz_types::input::actions::Action;

pub fn exercise_test_destination(socket: &std::path::Path, deadline: Instant) -> Result<Vec<u8>> {
    let args = Default::default();
    let mut config = Default::default();
    let mut sessions =
        crate::session_switch::Sessions::new(None, "source".into(), &args, &mut config);
    let mut prepared =
        crate::session_switch::commit_test_destination(&mut sessions, socket, deadline)?;
    let transport = get_cli_client_os_input().context("create destination transport")?;
    let receiver = prepared.receiver.take().context("committed receiver")?;
    receiver
        .set_recv_timeout(Some(std::time::Duration::from_secs(15)))
        .context("bound fixture receive")?;
    transport
        .install_session_connection(
            prepared.sender.take().context("committed sender")?,
            receiver,
        )
        .context("install committed destination")?;
    let os = FakeClientOsApi::new().capturing_stdout();
    let nested = NestedReannounce::spawn(Box::new(os.clone()), "destination".into());
    nested.stop();
    let budget = Arc::new(RenderBudget::new(1 << 20));
    let (input, _control) = crate::input_handler::input_control_channel();
    let mut client = ClientLoop {
        signal_quit_supported: Default::default(),
        recovery: &mut None,
        send_client_instructions: None,
        sessions: &mut sessions,
        remote_host: None,
        os_input: &os,
        command_is_executing: &mut CommandIsExecuting::new(),
        stdin_ansi_parser: &Arc::new(Mutex::new(StdinAnsiParser::new())),
        send_input_control: &input,
        nested_reannounce: &nested,
        teardown: ClientTeardown::new(24, false),
        synchronised_output: None,
        render_budget: &budget,
        startup_started: Instant::now(),
        splash: None,
        screensaver: crate::screensaver::Screensaver::new(&Default::default(), &Default::default()),
    };
    let mut outcome = ClientOutcome {
        exit_msg: String::new(),
        exit_status: 0,
        reconnect_to_session: None,
        restart: None,
        prepared_connection: None,
    };
    let mut dispatch = |client: &mut ClientLoop, msg: ServerToClientMsg| -> Result<()> {
        if let ServerToClientMsg::Render { content } = &msg {
            budget.reserve(content.len())?;
        }
        anyhow::ensure!(
            !client.handle(msg.into(), None, &mut outcome, &mut SlowWarnings::default()),
            "unexpected exit"
        );
        Ok(())
    };
    // The server issued this control after it dispatched every stale entry.
    anyhow::ensure!(
        matches!(
            prepared.buffered.pop_front(),
            Some((ServerToClientMsg::ConfigFileUpdated, _))
        ),
        "old output reached preparation"
    );
    anyhow::ensure!(prepared.buffered.len() == 1, "unexpected prepared messages");
    dispatch(
        &mut client,
        prepared.buffered.pop_front().context("first repaint")?.0,
    )?;
    transport.send_to_server(ClientToServerMsg::Action {
        action: Action::WriteChars {
            chars: "committed".into(),
        },
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    });

    let saver = client
        .screensaver
        .as_mut()
        .context("screensaver disabled")?;
    let now = saver.wake_at(Instant::now())?.context("idle deadline")?;
    saver.tick(&os, None, now, true)?;
    anyhow::ensure!(
        crate::screensaver::record_input(
            &saver.activity,
            &InputInstruction::KeyEvent(InputEvent::Paste("wake".into()), vec![]),
            now
        )?,
        "saver was not active"
    );
    anyhow::ensure!(
        saver.tick(&os, None, now, true)?,
        "wake did not request repaint"
    );
    transport.send_to_server(ClientToServerMsg::RequestRepaint);
    let mut crossed_barrier = false;
    let mut repainted = false;
    loop {
        let (msg, _) = transport
            .recv_from_server()
            .msg()
            .context("destination stopped before restoration")?;
        if matches!(msg, ServerToClientMsg::ConfigFileUpdated) {
            anyhow::ensure!(
                client.screensaver.as_ref().unwrap().wake_at(now)?.is_none(),
                "stale completion ended restoration"
            );
            crossed_barrier = true;
        }
        let complete = matches!(msg, ServerToClientMsg::RepaintComplete);
        if matches!(msg, ServerToClientMsg::Render { .. }) {
            anyhow::ensure!(crossed_barrier, "stale render during restore");
            repainted = true;
        }
        dispatch(&mut client, msg)?;
        if complete {
            anyhow::ensure!(
                crossed_barrier
                    && repainted
                    && client.screensaver.as_ref().unwrap().wake_at(now)?.is_some(),
                "completion arrived before the owner's repaint"
            );
            return Ok(os.stdout_bytes());
        }
    }
}
