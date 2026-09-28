use super::*;

const LEARN_MORE: &str = "<ENTER> - Learn More";

/// The help line as `render` prints it for the page's current selection.
fn help_line(page: &mut Page) -> String {
    page.help_text()
        .expect("every page has a help line")
        .content()
        .to_owned()
}

fn release_notes_main_screen() -> Page {
    Page::new_main_screen(
        "0.45.0".to_owned(),
        true,
        Rc::new(RefCell::new(KeybindingState::default())),
    )
}

/// A hoverable component whose hit area is `0..10` on row `y`. Clicking it, or `<ENTER>`
/// while it is selected, opens a blank page.
fn hoverable_component(y: usize) -> ActiveComponent {
    let mut component = ActiveComponent::new(TextOrCustomRender::CustomRender(
        Box::new(|_, _| 10),
        Box::new(|| 10),
    ))
    .with_hover(TextOrCustomRender::Text(Text::new("hover")))
    .with_left_click_action(ClickAction::new_change_page(Page::new));
    component.render(0, y, 1, 10, false);
    component
}

#[test]
fn mouse_hover_only_renders_when_selection_or_help_changes() {
    let mut page = Page::new()
        .with_bulletin_list(
            BulletinList::new(Text::new("menu")).with_items(vec![hoverable_component(0)]),
        )
        .with_paragraph(vec![ComponentLine::new(vec![hoverable_component(1)])]);

    assert!(page.handle_mouse_hover(1, 0));
    assert!(page.menu_item_is_selected());
    assert!(!page.handle_mouse_hover(2, 0));
    assert!(page.handle_mouse_hover(1, 1));
    assert!(!page.menu_item_is_selected());
    assert!(!page.handle_mouse_hover(2, 1));
    assert!(page.handle_mouse_hover(20, 1));
    assert!(!page.has_hover);
    assert!(!page.handle_mouse_hover(20, 1));

    page.set_active_selectable(0);
    assert!(
        page.handle_mouse_hover(20, 1),
        "clear a keyboard selection too"
    );
    assert_eq!(page.position_of_active_selectable(), None);
}

#[test]
fn keyboard_selection_offers_enter_until_it_is_cleared() {
    let mut page = release_notes_main_screen();
    let down = KeyWithModifier::new(BareKey::Down);
    let up = KeyWithModifier::new(BareKey::Up);
    assert!(
        !help_line(&mut page).contains(LEARN_MORE),
        "nothing is selected yet"
    );

    assert!(page.handle_key(down.clone()));
    assert!(
        help_line(&mut page).contains(LEARN_MORE),
        "<Down> selects the first item: {}",
        help_line(&mut page)
    );
    assert!(page.handle_key(down));
    assert!(help_line(&mut page).contains(LEARN_MORE));
    assert!(page.handle_key(up.clone()));
    assert!(help_line(&mut page).contains(LEARN_MORE));

    assert!(page.handle_key(up));
    assert!(
        !help_line(&mut page).contains(LEARN_MORE),
        "<Up> from the first item clears the selection: {}",
        help_line(&mut page)
    );
    assert!(page.handle_selection().is_none());
}

#[test]
fn clearing_a_hovered_item_with_the_keyboard_withdraws_enter() {
    let mut page = Page::new()
        .with_bulletin_list(
            BulletinList::new(Text::new("menu")).with_items(vec![hoverable_component(0)]),
        )
        .with_help(Box::new(|menu_item_is_selected| {
            main_screen_help_text(menu_item_is_selected, false)
        }));

    assert!(page.handle_mouse_hover(1, 0));
    assert!(help_line(&mut page).contains(LEARN_MORE));

    assert!(page.handle_key(KeyWithModifier::new(BareKey::Up)));
    assert!(
        !help_line(&mut page).contains(LEARN_MORE),
        "<Up> from the hovered first item clears the selection: {}",
        help_line(&mut page)
    );
    assert!(page.handle_selection().is_none(), "<ENTER> opens nothing");
}

#[test]
fn title_highlights_the_product_and_version() {
    for (is_release_notes, expected_title) in [
        (true, "Hi there, welcome to Yinz 0.45.0!"),
        (false, "Yinz 0.45.0"),
    ] {
        let title = main_screen_title("0.45.0".to_owned(), is_release_notes);
        assert_eq!(title.content(), expected_title);
        let characters: Vec<char> = title.content().chars().collect();
        let highlighted: Vec<String> = title
            .indices()
            .iter()
            .map(|level| level.iter().map(|index| characters[*index]).collect())
            .collect();
        assert_eq!(highlighted, ["", "", "Yinz 0.45.0"]);
    }
}
