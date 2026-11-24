use super::*;

#[tokio::test]
async fn split_input_completion_waits_for_all_targets_and_preserves_failures() {
    for count in [0, 1, 3] {
        for failed in [false, true] {
            let (tx, mut rx) = oneshot::channel();
            let mut parts = NotificationEnd::new_client_input(tx).split_for_input(count);
            assert_eq!(parts.len(), count);
            for part in &parts {
                assert!(part.requires_input_barrier());
            }
            if count > 0 {
                let mut first = parts.remove(0);
                if failed {
                    first.set_error_message("paste failed".into());
                }
                drop(first);
                if !parts.is_empty() {
                    assert!(matches!(
                        rx.try_recv(),
                        Err(oneshot::error::TryRecvError::Empty)
                    ));
                }
            }
            drop(parts);
            let result = tokio::time::timeout(yinz_test_support::default_timeout(), rx)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                result.error_message.as_deref(),
                (failed && count > 0).then_some("paste failed")
            );
        }
    }
}

#[test]
fn required_confirmation_reports_its_failure_unless_confirmed_or_explained() {
    let outcome = |confirm: bool, error: Option<&str>| {
        let (tx, rx) = oneshot::channel();
        let mut completion = NotificationEnd::new(tx);
        completion.require_confirmation("save never reached the disk".into());
        // Clones carry the requirement but answer nobody.
        drop(completion.clone());
        if confirm {
            completion.confirm();
        }
        if let Some(error) = error {
            completion.set_error_message(error.into());
        }
        drop(completion);
        let result = rx.blocking_recv().unwrap();
        assert!(result.confirmed);
        result.error_message
    };
    assert_eq!(
        outcome(false, None).as_deref(),
        Some("save never reached the disk")
    );
    assert_eq!(outcome(true, None), None);
    // An owner's specific error wins over the generic one.
    assert_eq!(
        outcome(false, Some("disk full")).as_deref(),
        Some("disk full")
    );
    assert_eq!(
        outcome(true, Some("disk full")).as_deref(),
        Some("disk full")
    );
}

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
        switch_source_retained: false,
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

#[test]
fn startup_completion_waits_for_both_owners_and_keeps_screen_placement_metadata() {
    use yinz_types::internal_ids::{TabId, TerminalId};
    for placement_first in [false, true] {
        let (tx, mut rx) = oneshot::channel();
        let mut original = NotificationEnd::new(tx);
        original.wait_for_agent_start = true;
        let (mut placement, startup) = original.split_for_startup();
        placement.created_tab_id = Some(TabId::new(8));
        placement.set_affected_pane_id(PaneId::Terminal(TerminalId::new(42)));
        placement.complete_pane_placement(TabId::new(8));
        if placement_first {
            drop(placement);
            assert!(rx.try_recv().is_err());
            drop(startup);
        } else {
            drop(startup);
            assert!(rx.try_recv().is_err());
            drop(placement);
        }
        let result = rx.blocking_recv().unwrap();
        assert_eq!(result.affected_tab_id, Some(TabId::new(8)));
        assert_eq!(result.created_tab_id, Some(TabId::new(8)));
        assert_eq!(
            result.affected_pane_id,
            Some(PaneId::Terminal(TerminalId::new(42)))
        );
        assert!(result.error_message.is_none());
    }
}
