//! The event loop that drives a client attached to a *remote* session.
//!
//! Unlike the local client, which talks to the server over an IPC socket and
//! fans work out across several OS threads, the remote client multiplexes
//! everything onto one `tokio::select!`: bytes from stdin go out over the
//! terminal WebSocket, terminal output comes back and is painted, resize
//! signals and control messages travel over a second control WebSocket, and
//! nested-session frames are peeled off stdin before the rest is forwarded.

use crate::os_input_output::{AsyncSignals, AsyncStdin, ClientOsApi};
use crate::stdin_ansi_parser::SyncOutput;
use crate::terminal_escapes::{announce_nested, write_synchronised};
use crate::web_client::control_message::{
    WebClientToWebServerControlMessage, WebClientToWebServerControlMessagePayload,
    WebServerToWebClientControlMessage,
};
use crate::{remote_attach, RemoteClientError};
use futures_util::{SinkExt, StreamExt};
use std::io::Write;
use tokio_tungstenite::tungstenite::Message;
use zellij_utils::{data::ConnectToSession, nested_session, pane_size::Size};

pub async fn run_remote_client_terminal_loop(
    os_input: Box<dyn ClientOsApi>,
    mut connections: remote_attach::WebSocketConnections,
    nested_session_name: Option<String>,
) -> Result<Option<ConnectToSession>, RemoteClientError> {
    let synchronised_output = match os_input.env_variable("TERM").as_deref() {
        Some("alacritty") => Some(SyncOutput::DCS),
        _ => None,
    };

    let mut async_stdin: Box<dyn AsyncStdin> = os_input.get_async_stdin_reader();
    let mut async_signals: Box<dyn AsyncSignals> = os_input
        .get_async_signal_listener()
        .map_err(RemoteClientError::IoError)?;

    let create_resize_message = |size: Size| {
        Message::Text(
            serde_json::to_string(&WebClientToWebServerControlMessage {
                web_client_id: connections.web_client_id.clone(),
                payload: WebClientToWebServerControlMessagePayload::TerminalResize(size),
            })
            .unwrap()
            .into(),
        )
    };

    // send size on startup
    let new_size = os_input.get_terminal_size();
    if let Err(e) = connections
        .control_ws
        .send(create_resize_message(new_size))
        .await
    {
        log::error!("Failed to send resize message: {}", e);
    }

    let mut nested_frame_extractor = nested_session::NestedFrameExtractor::new();
    let mut last_heard_from_host = std::time::Instant::now();
    let mut reannounce_check = tokio::time::interval(std::time::Duration::from_millis(
        nested_session::reannounce_check_interval_ms(),
    ));
    reannounce_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Handle stdin input
            result = async_stdin.read() => {
                match result {
                    Ok(buf) if !buf.is_empty() => {
                        let (cleaned, nested_frames) = nested_frame_extractor.extract(&buf);
                        for payload_bytes in nested_frames {
                            last_heard_from_host = std::time::Instant::now();
                            match nested_session::decode_payload(&payload_bytes) {
                                Some(nested_session::NestedSessionMessage::Ping) => {
                                    let mut stdout = os_input.get_stdout_writer();
                                    let _ = stdout.write_all(&nested_session::encode_frame(
                                        &nested_session::NestedSessionMessage::Pong,
                                    ));
                                    let _ = stdout.flush();
                                },
                                Some(_) => {
                                    let control_msg = Message::Text(
                                        serde_json::to_string(&WebClientToWebServerControlMessage {
                                            web_client_id: connections.web_client_id.clone(),
                                            payload: WebClientToWebServerControlMessagePayload::NestedSessionFrameFromHost {
                                                payload_bytes,
                                            },
                                        })
                                        .unwrap()
                                        .into(),
                                    );
                                    if let Err(e) = connections.control_ws.send(control_msg).await {
                                        log::error!("Failed to forward nested session frame over control WebSocket: {}", e);
                                    }
                                },
                                None => {},
                            }
                        }
                        if !cleaned.is_empty() {
                            if let Err(e) = connections.terminal_ws.send(Message::Binary(cleaned.into())).await {
                                log::error!("Failed to send stdin to terminal WebSocket: {}", e);
                                break;
                            }
                        }
                    }
                    Ok(_) => {
                        // Empty buffer means EOF
                        break;
                    }
                    Err(e) => {
                        log::error!("Error reading from stdin: {}", e);
                        break;
                    }
                }
            }

            _ = reannounce_check.tick() => {
                if let Some(session_name) = &nested_session_name {
                    if last_heard_from_host.elapsed()
                        >= std::time::Duration::from_millis(nested_session::reannounce_silence_ms())
                    {
                        announce_nested(&*os_input, session_name);
                    }
                }
            }

            // Handle signals
            Some(signal) = async_signals.recv() => {
                match signal {
                    crate::os_input_output::SignalEvent::Resize => {
                        let new_size = os_input.get_terminal_size();
                        if let Err(e) = connections.control_ws.send(create_resize_message(new_size)).await {
                            log::error!("Failed to send resize message: {}", e);
                            break;
                        }
                    }
                    crate::os_input_output::SignalEvent::Quit => {
                        break;
                    }
                }
            }

            // Handle terminal messages
            terminal_msg = connections.terminal_ws.next() => {
                match terminal_msg {
                    Some(Ok(Message::Text(text))) => {
                        let mut stdout = os_input.get_stdout_writer();
                        write_synchronised(&mut *stdout, synchronised_output, text.as_bytes());
                    }
                    Some(Ok(Message::Binary(data))) => {
                        let mut stdout = os_input.get_stdout_writer();
                        write_synchronised(&mut *stdout, synchronised_output, &data);
                    }
                    Some(Ok(Message::Close(_))) => {
                        break;
                    }
                    Some(Err(e)) => {
                        log::error!("Error: {}", e);
                        break;
                    }
                    None => {
                        log::error!("Received empty message from web server");
                        break;
                    }
                    _ => {}
                }
            }

            control_msg = connections.control_ws.next() => {
                match control_msg {
                    Some(Ok(Message::Text(msg))) => {
                        let deserialized_msg: Result<WebServerToWebClientControlMessage, _> =
                            serde_json::from_str(&msg);
                        match deserialized_msg {
                            Ok(WebServerToWebClientControlMessage::SetConfig(..)) => {
                                // no-op
                            }
                            Ok(WebServerToWebClientControlMessage::QueryTerminalSize) => {
                                let new_size = os_input.get_terminal_size();
                                if let Err(e) = connections.control_ws.send(create_resize_message(new_size)).await {
                                    log::error!("Failed to send resize message: {}", e);
                                }
                            }
                            Ok(WebServerToWebClientControlMessage::Log { lines }) => {
                                for line in lines {
                                    log::info!("{}", line);
                                }
                            }
                            Ok(WebServerToWebClientControlMessage::LogError { lines }) => {
                                for line in lines {
                                    log::error!("{}", line);
                                }
                            }
                            Ok(WebServerToWebClientControlMessage::SwitchedSession{ .. }) => {
                                // no-op
                            }
                            Ok(WebServerToWebClientControlMessage::SetSoftKeyboard{ .. }) => {
                                // no-op
                            }
                            Ok(WebServerToWebClientControlMessage::MobileState{ .. }) => {
                                // no-op
                            }
                            Err(e) => {
                                log::debug!("Ignoring unrecognized control message: {}", e);
                            }
                        }

                    }
                    Some(Ok(Message::Close(_))) => {
                        break;
                    }
                    Some(Err(e)) => {
                        log::error!("{}", e);
                        break;
                    }
                    None => break,
                    _ => {}
                }
            }

        }
    }

    Ok(None)
}
