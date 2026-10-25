use super::*;

#[test]
fn the_default_menu_identifies_every_item_by_position() {
    let menu = ContextMenu::defaults();
    for (index, item) in menu.items().iter().enumerate() {
        assert_eq!(item.id, Some(ContextMenuItemId::new(index as u64)));
    }
}

#[test]
fn the_items_a_plugin_sees_carry_ids_but_no_actions() {
    let menu = ContextMenu::defaults();
    let for_plugin = menu.items_for_plugin();
    assert_eq!(for_plugin.len(), menu.items().len());
    for (sent, configured) in for_plugin.iter().zip(menu.items()) {
        assert_eq!(sent.id, configured.id);
        assert_eq!(sent.label, configured.label);
        assert!(
            sent.actions.is_empty(),
            "actions must not cross the plugin boundary - see data::ContextMenuItem"
        );
    }
}
