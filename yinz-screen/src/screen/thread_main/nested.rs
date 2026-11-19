use super::*;
use yinz_bus::ClientId;

pub(super) fn focus_host_session(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    let payload =
        nested_session::encode_payload(&NestedSessionMessage::FocusHost { direction: None });
    let _ =
        screen
            .core
            .bus
            .senders
            .send_to_server(ServerInstruction::EmitNestedSessionFrameToClient(
                client_id, payload,
            ));
    Ok(())
}

pub(super) fn toggle_host_fullscreen(screen: &mut Screen, client_id: ClientId) -> Result<()> {
    screen.nested.own_fullscreen_requested = !screen.nested.own_fullscreen_requested;
    let fullscreen = screen.nested.own_fullscreen_requested;
    let payload =
        nested_session::encode_payload(&NestedSessionMessage::ToggleHostFullscreen { fullscreen });
    let _ =
        screen
            .core
            .bus
            .senders
            .send_to_server(ServerInstruction::EmitNestedSessionFrameToClient(
                client_id, payload,
            ));
    Ok(())
}
