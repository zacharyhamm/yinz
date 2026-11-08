use super::*;
use yinz_types::internal_ids::TabId;

const FIRST: ClientId = ClientId::new(1);
const SECOND: ClientId = ClientId::new(2);

fn changes(capture: &ThemeCapture) -> Vec<(ClientId, Option<String>)> {
    capture
        .drain_plugin_updates()
        .into_iter()
        .filter_map(|(plugin, recipient, event)| {
            if let Event::ClientTimezoneChanged {
                client_id,
                timezone,
            } = event
            {
                assert_eq!(plugin, None);
                assert_eq!(
                    recipient, None,
                    "subject must not become the recipient/actor"
                );
                Some((client_id, timezone))
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn client_timezone_startup_order_duplicates_invalidation_and_recycled_ids() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    screen.set_client_timezone(FIRST, Some("America/Chicago"));
    assert_eq!(
        screen.client_timezone(FIRST).report(),
        None,
        "provisional client is not queryable"
    );
    assert!(changes(&capture).is_empty());
    new_tab(&mut screen, 1, TabId::new(0));
    assert_eq!(
        changes(&capture),
        vec![(FIRST, Some("America/Chicago".into()))]
    );
    screen.add_client(SECOND).unwrap();
    assert_eq!(changes(&capture), vec![(SECOND, None)]);
    screen.set_client_timezone(SECOND, Some("Asia/Kolkata"));
    screen.set_client_timezone(SECOND, Some("Asia/Kolkata"));
    screen.set_client_timezone(FIRST, Some("America/Chicago"));
    assert_eq!(
        changes(&capture),
        vec![(SECOND, Some("Asia/Kolkata".into()))]
    );
    let metadata = screen.get_layout_metadata(None, None);
    assert_eq!(
        metadata.client_timezone(FIRST).as_deref(),
        Some("America/Chicago")
    );
    assert_eq!(
        metadata.client_timezone(SECOND).as_deref(),
        Some("Asia/Kolkata")
    );
    screen.set_client_timezone(SECOND, Some("invalid/zone"));
    screen.set_client_timezone(SECOND, None);
    assert_eq!(changes(&capture), vec![(SECOND, None)]);
    screen.set_client_timezone(SECOND, Some("Asia/Kolkata"));
    changes(&capture);
    screen.remove_client(SECOND).unwrap();
    assert_eq!(screen.client_timezone(SECOND).report(), None);
    assert_eq!(changes(&capture), vec![(SECOND, None)]);
    screen.add_client(SECOND).unwrap();
    assert_eq!(changes(&capture), vec![(SECOND, None)]);
    screen.remove_client(SECOND).unwrap();
    changes(&capture);
    screen.set_client_timezone(SECOND, Some("Asia/Kolkata"));
    screen.add_client(SECOND).unwrap();
    assert_eq!(
        changes(&capture),
        vec![(SECOND, Some("Asia/Kolkata".into()))]
    );
    // Earlier listing snapshots must retain their reports through later mutations/enrichment.
    assert_eq!(
        metadata.client_timezone(SECOND).as_deref(),
        Some("Asia/Kolkata")
    );
    assert_eq!(
        screen.client_timezone(FIRST).report(),
        Some("America/Chicago")
    );
}

#[test]
fn client_timezone_failed_registration_clears_provisional_report_without_attachment_event() {
    let (mut screen, capture) = create_new_screen_with_theme_capture(Size { cols: 80, rows: 20 });
    screen.set_client_timezone(SECOND, Some("Asia/Kolkata"));
    assert!(screen
        .register_client_transaction(SECOND, None, None, true)
        .is_err());
    assert!(!screen.client_timezones.contains_key(&SECOND));
    assert_eq!(screen.client_timezone(SECOND).report(), None);
    assert!(changes(&capture).is_empty());
}
