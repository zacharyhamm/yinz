use super::*;
use yinz_bus::ClientId;
use yinz_types::data::NotificationInfo;

pub(super) fn list_notifications(
    screen: &mut Screen,
    response_channel: crossbeam::channel::Sender<Vec<NotificationInfo>>,
) -> Result<()> {
    let _ = response_channel.send(
        screen
            .notices
            .notification_history
            .iter()
            .map(|record| record.info.clone())
            .collect(),
    );
    Ok(())
}

pub(super) fn desktop_notification_response(
    screen: &mut Screen,
    raw_bytes: Vec<u8>,
    client_id: ClientId,
) -> Result<()> {
    if screen.activate_policy_desktop(&raw_bytes, client_id) {
        screen.render()?;
    } else if let Some(response) = denormalize_notification_response(&raw_bytes) {
        let pane_id = PaneId::Terminal(response.terminal_id);
        if response.focus_pane {
            screen.consume_pane_desktop_notification(pane_id, &raw_bytes, Instant::now());
        }
        if response.forward_to_pane
            && let Some(tab) = screen.tab_containing_pane_mut(pane_id)
        {
            tab.write_to_pane_id(&None, response.bytes, false, pane_id, None, None)
                .non_fatal();
        }
        if response.focus_pane {
            screen
                .focus_notification_pane(pane_id, false, client_id)
                .non_fatal();
        }
        screen.render()?;
        screen.log_and_report_session_state()?;
    }
    Ok(())
}
