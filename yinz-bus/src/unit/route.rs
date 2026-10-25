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
        confirmed: true,
        agent_tab: None,
        exit_status: None,
        affected_pane_id: None,
        affected_tab_id: Some(TabId::new(123)),
        created_tab_id: None,
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
