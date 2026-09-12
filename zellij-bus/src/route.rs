//! Completion payloads carried by the thread bus. Action dispatch lives in `zellij-routing`.
use crate::PaneId;
use tokio::sync::oneshot;
use zellij_types::{data::UnblockCondition, internal_ids::TabId};

#[derive(Debug, Clone)]
pub struct ActionCompletionResult {
    pub exit_status: Option<i32>,
    pub affected_pane_id: Option<PaneId>,
    pub affected_tab_id: Option<TabId>,
    pub error_message: Option<String>,
    pub stdout_message: Option<String>,
}

// This is used to wait for actions that span multiple threads until they logically end
// dropping this struct sends a notification through the oneshot channel to the receiver, letting
// it know the action is ended and thus releasing it
//
// Note: Cloning this struct DOES NOT clone that internal receiver, it only implements Clone so
// that it can be included in various other larger structs - DO NOT RELY ON CLONING IT!
pub struct NotificationEnd {
    channel: Option<oneshot::Sender<ActionCompletionResult>>,
    exit_status: Option<i32>,
    unblock_condition: Option<UnblockCondition>,
    affected_pane_id: Option<PaneId>, // optional payload of the pane id affected by this action
    affected_tab_id: Option<TabId>,   // optional payload of the tab id affected by this action
    error_message: Option<String>,
    stdout_message: Option<String>,
    /// Handles answered together with this one, with its payload: see `join`.
    followers: Vec<NotificationEnd>,
}

// Hand-written so that `followers`, empty on every handle that was not `join`ed, does not show
// up in the many snapshots that hold a `NotificationEnd`.
impl std::fmt::Debug for NotificationEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("NotificationEnd");
        s.field("channel", &self.channel)
            .field("exit_status", &self.exit_status)
            .field("unblock_condition", &self.unblock_condition)
            .field("affected_pane_id", &self.affected_pane_id)
            .field("affected_tab_id", &self.affected_tab_id)
            .field("error_message", &self.error_message)
            .field("stdout_message", &self.stdout_message);
        if !self.followers.is_empty() {
            s.field("followers", &self.followers);
        }
        s.finish()
    }
}

impl Clone for NotificationEnd {
    fn clone(&self) -> Self {
        // Always clone as None - only the original holder should signal completion
        NotificationEnd {
            channel: None,
            exit_status: self.exit_status,
            unblock_condition: self.unblock_condition,
            affected_pane_id: self.affected_pane_id,
            affected_tab_id: self.affected_tab_id,
            error_message: self.error_message.clone(),
            stdout_message: self.stdout_message.clone(),
            followers: vec![],
        }
    }
}

impl NotificationEnd {
    pub fn new(sender: oneshot::Sender<ActionCompletionResult>) -> Self {
        NotificationEnd {
            channel: Some(sender),
            exit_status: None,
            unblock_condition: None,
            affected_pane_id: None,
            affected_tab_id: None,
            error_message: None,
            stdout_message: None,
            followers: vec![],
        }
    }

    pub fn new_with_condition(
        sender: oneshot::Sender<ActionCompletionResult>,
        unblock_condition: UnblockCondition,
    ) -> Self {
        NotificationEnd {
            channel: Some(sender),
            exit_status: None,
            unblock_condition: Some(unblock_condition),
            affected_pane_id: None,
            affected_tab_id: None,
            error_message: None,
            stdout_message: None,
            followers: vec![],
        }
    }

    pub fn set_exit_status(&mut self, exit_status: i32) {
        self.exit_status = Some(exit_status);
    }

    pub fn set_affected_pane_id(&mut self, pane_id: PaneId) {
        self.affected_pane_id = Some(pane_id);
    }

    pub fn set_affected_tab_id(&mut self, tab_id: TabId) {
        self.affected_tab_id = Some(tab_id);
    }

    pub fn set_error_message(&mut self, message: String) {
        self.error_message = Some(message);
    }

    pub fn is_cancelled(&self) -> bool {
        self.channel
            .as_ref()
            .is_some_and(|channel| channel.is_closed())
    }

    pub fn set_stdout_message(&mut self, message: String) {
        self.stdout_message = Some(message);
    }

    pub fn unblock_condition(&self) -> Option<UnblockCondition> {
        self.unblock_condition
    }

    /// One handle standing for several waiters. When several requests were satisfied by running
    /// one of them (a run of identical plugin reloads collapsed into one), the survivor's handle
    /// is passed along the same path as before and the others ride inside it: they are answered
    /// when it is, with whatever it was told - the pane id an `AddPlugin` set on it, not the
    /// empty result a dropped handle reports. With no `primary`, the first of `others` stands in
    /// for it; with nothing at all, `None`.
    pub fn join(primary: Option<NotificationEnd>, others: Vec<NotificationEnd>) -> Option<Self> {
        let mut others = others.into_iter();
        let mut primary = primary.or_else(|| others.next())?;
        primary.followers.extend(others);
        Some(primary)
    }
}

impl Drop for NotificationEnd {
    fn drop(&mut self) {
        // Followers are fields, dropped after this body: each sends the copy it is given here.
        for follower in self.followers.iter_mut() {
            follower.exit_status = self.exit_status;
            follower.affected_pane_id = self.affected_pane_id;
            follower.affected_tab_id = self.affected_tab_id;
            follower.error_message = self.error_message.clone();
            follower.stdout_message = self.stdout_message.clone();
        }
        if let Some(tx) = self.channel.take() {
            let result = ActionCompletionResult {
                exit_status: self.exit_status,
                affected_pane_id: self.affected_pane_id,
                affected_tab_id: self.affected_tab_id,
                error_message: self.error_message.take(),
                stdout_message: self.stdout_message.take(),
            };
            let _ = tx.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_end_sets_affected_tab_id() {
        let (tx, rx) = oneshot::channel();
        let mut notification_end = NotificationEnd::new(tx);

        notification_end.set_affected_tab_id(TabId::new(42));

        drop(notification_end);

        let result = rx.blocking_recv().unwrap();
        assert_eq!(result.affected_tab_id, Some(TabId::new(42)));
    }

    #[test]
    fn test_notification_end_default_affected_tab_id_none() {
        let (tx, rx) = oneshot::channel();
        let notification_end = NotificationEnd::new(tx);

        drop(notification_end);

        let result = rx.blocking_recv().unwrap();
        assert_eq!(result.affected_tab_id, None);
    }

    #[test]
    fn test_action_completion_result_includes_tab_id() {
        let result = ActionCompletionResult {
            exit_status: None,
            affected_pane_id: None,
            affected_tab_id: Some(TabId::new(123)),
            error_message: None,
            stdout_message: None,
        };

        assert_eq!(result.affected_tab_id, Some(TabId::new(123)));
    }

    #[test]
    fn test_notification_end_with_pane_and_tab_ids() {
        let (tx, rx) = oneshot::channel();
        let mut notification_end = NotificationEnd::new(tx);

        notification_end.set_affected_pane_id(PaneId::Terminal(crate::TerminalId::new(10)));
        notification_end.set_affected_tab_id(TabId::new(5));

        drop(notification_end);

        let result = rx.blocking_recv().unwrap();
        assert_eq!(
            result.affected_pane_id,
            Some(PaneId::Terminal(crate::TerminalId::new(10)))
        );
        assert_eq!(result.affected_tab_id, Some(TabId::new(5)));
    }

    #[test]
    fn test_notification_end_clone_does_not_copy_channel() {
        let (tx, _rx) = oneshot::channel();
        let mut notification_end = NotificationEnd::new(tx);
        notification_end.set_affected_tab_id(TabId::new(99));

        let cloned = notification_end.clone();

        // Verify the clone has the same data
        assert_eq!(cloned.affected_tab_id, Some(TabId::new(99)));
        // But channel should be None (as per the Clone implementation comment)
        assert!(cloned.channel.is_none());
    }
}
