use super::*;

fn client(id: u32) -> ClientId {
    ClientId::new(id)
}

#[test]
fn an_attaching_client_takes_the_lowest_free_slot_and_keeps_it() {
    let mut attached = AttachedClients::default();
    for id in [5, 9, 40] {
        assert!(attached.insert(client(id)));
    }
    assert_eq!(attached.color_slot(&client(9)), ColorSlot::new(2));
    assert!(attached.remove(&client(5)));
    assert!(!attached.insert(client(9)), "already attached");
    assert_eq!(attached.color_slot(&client(9)), ColorSlot::new(2));
    attached.insert(client(41));
    assert_eq!(attached.color_slot(&client(41)), ColorSlot::new(1));
    assert_eq!(attached.color_slot(&client(40)), ColorSlot::new(3));
    assert_eq!(attached.color_slot(&client(5)), None);
}

#[test]
fn clients_past_ten_have_no_slot_and_do_not_take_a_freed_one() {
    let mut attached: AttachedClients = (1..=11).map(client).collect();
    assert_eq!(attached.color_slot(&client(10)), ColorSlot::new(10));
    assert_eq!(attached.color_slot(&client(11)), None);
    attached.remove(&client(3));
    assert_eq!(attached.color_slot(&client(11)), None);
    attached.insert(client(12));
    assert_eq!(attached.color_slot(&client(12)), ColorSlot::new(3));
}

#[test]
fn a_snapshot_answers_like_the_set_it_was_taken_from() {
    let attached: AttachedClients = [client(70), client(3)].into_iter().collect();
    let snapshot = attached.snapshot();
    assert!(snapshot.multiple_users());
    assert_eq!(snapshot.color_slot(client(3)), ColorSlot::new(1));
    assert_eq!(snapshot.color_slot(client(70)), ColorSlot::new(2));
    assert_eq!(snapshot.color_slot(client(4)), None);
    let alone: AttachedClients = [client(1)].into_iter().collect();
    assert!(!alone.snapshot().multiple_users());
}

#[test]
fn color_slots_are_one_to_ten_and_serialize_as_numbers() {
    assert_eq!(ColorSlot::new(0), None);
    assert_eq!(ColorSlot::new(11), None);
    let slot = ColorSlot::new(10).unwrap();
    assert_eq!(serde_json::to_string(&slot).unwrap(), "10");
    assert_eq!(serde_json::from_str::<ColorSlot>("10").unwrap(), slot);
    assert!(serde_json::from_str::<ColorSlot>("11").is_err());
    let colors = crate::style::MultiplayerColors {
        player_10: PaletteColor::Rgb((1, 2, 3)),
        ..Default::default()
    };
    assert_eq!(slot.colors(colors).0, PaletteColor::Rgb((1, 2, 3)));
}
