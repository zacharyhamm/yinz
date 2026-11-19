use yinz_types::agent::tracking::AgentTrackingHandoff;

/// Rebase evidence from the source pane onto the destination's local input counter.
pub fn remap_input(tracking: &mut AgentTrackingHandoff, destination: u64) {
    let source = tracking.input_revision;
    let remap = |revision| {
        if revision == source {
            destination
        } else {
            destination.wrapping_sub(1)
        }
    };
    if let Some(lifecycle) = &mut tracking.lifecycle {
        lifecycle.input_revision = lifecycle.input_revision.map(remap);
        // Permission waits are thresholds. Only input delivered after adoption can clear them.
        for notification in lifecycle.claude_notifications.values_mut() {
            notification.permission_revision =
                notification.permission_revision.map(|_| destination);
        }
    }
    if let Some(dialog) = &mut tracking.dialog_input {
        dialog.input_revision = remap(dialog.input_revision);
    }
    if let Some(permission) = &mut tracking.permission_wait {
        permission.input_revision = destination;
    }
    tracking.input_revision = destination;
}

#[cfg(test)]
#[path = "unit/handoff.rs"]
mod tests;
