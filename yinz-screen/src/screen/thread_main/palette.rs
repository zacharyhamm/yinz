use super::*;
use yinz_bus::ClientId;
use yinz_bus::PaneId;
use yinz_types::geometry::Direction;
use yinz_types::internal_ids::TabId;

pub(super) fn move_command_palette_pane(
    screen: &mut Screen,
    pane_id: PaneId,
    tab_id: TabId,
    direction: Option<Direction>,
    client_id: ClientId,
) -> Result<()> {
    if let Err(error) = screen.move_command_palette_pane(pane_id, tab_id, direction, client_id) {
        screen
            .core
            .bus
            .senders
            .send_to_screen(ScreenInstruction::Notify(
                format!("Could not move pane: {error}"),
                None,
                None,
                None,
            ))
            .non_fatal();
    }
    screen.render()?;
    Ok(())
}
