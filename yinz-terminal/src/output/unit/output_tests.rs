use super::super::{
    ChangedLines, CharacterChunk, FloatingPanesStack, HostImageRecord, HostKittyState,
    KittyImageChunk, Output, OutputBuffer, SixelImageChunk,
};
use crate::panes::kitty_graphics::parser::{
    DecodedImage, KittyAction, KittyCommandParser, KittyFormat,
};
use crate::panes::kitty_graphics::store::{InternalImageId, KittyImageStore};
use crate::panes::sixel::SixelImageStore;
use crate::panes::terminal_character::AnsiCode;
use crate::panes::{LinkHandler, PaneId, Row, TerminalCharacter};
use crate::ClientId;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use yinz_types::internal_ids::TerminalId;
use yinz_types::kitty_placeholders::PLACEHOLDER;
use yinz_types::pane_size::{Dimension, PaneGeom, Size, SizeInPixels};

/// Helper to create a simple Output instance for testing
fn create_test_output() -> Output {
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        height: 20,
        width: 10,
    })));
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    Output::new(
        sixel_image_store,
        character_cell_size,
        styled_underlines,
        osc8_hyperlinks,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
    )
}

/// The same, but with the kitty host capability already known for `client_id`. `Output` reads
/// that map to decide whether a client gets image bytes at all, and therefore whether its
/// placeholder cells mean anything.
fn create_test_output_with_kitty_support(client_id: ClientId, supported: bool) -> Output {
    let kitty_host_capabilities = Rc::new(RefCell::new(HashMap::new()));
    kitty_host_capabilities
        .borrow_mut()
        .insert(client_id, supported);
    Output::new(
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            height: 20,
            width: 10,
        }))),
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        kitty_host_capabilities,
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
    )
}

/// Helper to create a simple CharacterChunk with text
fn create_character_chunk_from_str(text: &str, x: usize, y: usize) -> CharacterChunk {
    let terminal_chars: Vec<TerminalCharacter> = text.chars().map(TerminalCharacter::new).collect();
    CharacterChunk::new(terminal_chars, x, y)
}

/// Helper to create test clients
fn create_test_clients(count: usize) -> HashSet<ClientId> {
    (1..=count).map(|i| ClientId::new(i as u32)).collect()
}

/// Helper to create PaneGeom for FloatingPanesStack tests
fn create_pane_geom(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    PaneGeom {
        x,
        y,
        cols: Dimension::fixed(cols),
        rows: Dimension::fixed(rows),
        stacked: None,
        is_pinned: false,
        logical_position: None,
    }
}

#[test]
fn test_output_new() {
    let output = create_test_output();

    // Verify default state of all fields
    assert!(!output.is_dirty(), "New output should not be dirty");
    assert!(
        !output.has_rendered_assets(),
        "New output should not have rendered assets"
    );
}

#[test]
fn vte_only_clients_are_serialized_without_pane_registration() {
    for sized in [false, true] {
        let mut output = create_test_output();
        let client = ClientId::new(1);
        let other = ClientId::new(2);
        output.add_pre_vte_instruction_to_client(client, "before");
        output.add_post_vte_instruction_to_client(client, "after");
        output.add_pre_vte_instruction_to_multiple_clients([other].into_iter(), "before");
        output.add_post_vte_instruction_to_multiple_clients([other].into_iter(), "after");
        assert!(output.is_dirty());
        assert!(!output.has_rendered_assets());
        let serialized = if sized {
            output.serialize_with_size(None).unwrap()
        } else {
            output.serialize().unwrap()
        };
        for id in [client, other] {
            let bytes = &serialized[&id];
            assert!(bytes.starts_with("before"), "{bytes:?}");
            assert!(bytes.ends_with("after"), "{bytes:?}");
        }
        assert!(!output.is_dirty());
        assert!(output.serialize().unwrap().is_empty());
    }
}

#[test]
fn test_add_clients() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(3);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));

    output.add_clients(&client_ids, link_handler, None);

    // Verify that client_character_chunks has entries for all clients
    assert!(!output.is_dirty(), "Should not be dirty until chunks added");
}

#[test]
fn test_is_dirty_with_empty_output() {
    let output = create_test_output();
    assert!(!output.is_dirty(), "Empty output should not be dirty");
}

#[test]
fn test_is_dirty_with_character_chunks() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk = create_character_chunk_from_str("Hi", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    assert!(
        output.is_dirty(),
        "Output should be dirty after adding character chunks"
    );
}

#[test]
fn test_is_dirty_with_pre_vte_instructions() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    output.add_pre_vte_instruction_to_client(ClientId::new(1), "\u{1b}[?1049h");

    assert!(
        output.is_dirty(),
        "Output should be dirty after adding pre VTE instructions"
    );
}

#[test]
fn test_is_dirty_with_post_vte_instructions() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    output.add_post_vte_instruction_to_client(ClientId::new(1), "\u{1b}[?25h");

    assert!(
        output.is_dirty(),
        "Output should be dirty after adding post VTE instructions"
    );
}

#[test]
fn test_is_dirty_with_sixel_chunks() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let sixel_chunk = SixelImageChunk {
        cell_x: 0,
        cell_y: 0,
        sixel_image_pixel_x: 0,
        sixel_image_pixel_y: 0,
        sixel_image_pixel_width: 100,
        sixel_image_pixel_height: 100,
        sixel_image_id: 1,
    };
    output.add_sixel_image_chunks_to_client(ClientId::new(1), vec![sixel_chunk], None);

    assert!(
        output.is_dirty(),
        "Output should be dirty after adding sixel chunks"
    );
}

#[test]
fn test_has_rendered_assets_empty() {
    let output = create_test_output();
    assert!(
        !output.has_rendered_assets(),
        "Empty output should not have rendered assets"
    );
}

#[test]
fn test_has_rendered_assets_only_vte_instructions() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    output.add_pre_vte_instruction_to_client(ClientId::new(1), "\u{1b}[?25l");
    output.add_post_vte_instruction_to_client(ClientId::new(1), "\u{1b}[?25h");

    assert!(
        !output.has_rendered_assets(),
        "VTE instructions alone should not count as rendered assets"
    );
}

#[test]
fn test_has_rendered_assets_with_character_chunks() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk = create_character_chunk_from_str("Hello", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    assert!(
        output.has_rendered_assets(),
        "Character chunks should count as rendered assets"
    );
}

#[test]
fn test_has_rendered_assets_with_sixel_chunks() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let sixel_chunk = SixelImageChunk {
        cell_x: 0,
        cell_y: 0,
        sixel_image_pixel_x: 0,
        sixel_image_pixel_y: 0,
        sixel_image_pixel_width: 100,
        sixel_image_pixel_height: 100,
        sixel_image_id: 1,
    };
    output.add_sixel_image_chunks_to_client(ClientId::new(1), vec![sixel_chunk], None);

    assert!(
        output.has_rendered_assets(),
        "Sixel chunks should count as rendered assets"
    );
}

#[test]
fn test_serialize_empty() {
    let mut output = create_test_output();
    let result = output.serialize().unwrap();
    assert!(
        result.is_empty(),
        "Serializing empty output should return empty HashMap"
    );
}

#[test]
fn test_serialize_single_client_simple_text() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk = create_character_chunk_from_str("Hello", 5, 10);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    let result = output.serialize().unwrap();
    assert_eq!(result.len(), 1, "Should have one client in result");

    let client_output = result.get(&ClientId::new(1)).unwrap();
    // Verify contains goto instruction (y+1, x+1 for 1-indexed VTE)
    assert!(
        client_output.contains("\u{1b}[11;6H"),
        "Should contain goto instruction for position (5, 10)"
    );
    // Verify contains reset styles
    assert!(
        client_output.contains("\u{1b}[m"),
        "Should contain reset styles"
    );
    // Verify contains the text
    assert!(client_output.contains("Hello"), "Should contain the text");
}

#[test]
fn test_serialize_multiple_clients() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(2);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk1 = create_character_chunk_from_str("Hello", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk1], None)
        .unwrap();

    let chunk2 = create_character_chunk_from_str("World", 10, 10);
    output
        .add_character_chunks_to_client(ClientId::new(2), vec![chunk2], None)
        .unwrap();

    let result = output.serialize().unwrap();
    assert_eq!(result.len(), 2, "Should have two clients in result");

    let client1_output = result.get(&ClientId::new(1)).unwrap();
    assert!(
        client1_output.contains("Hello"),
        "Client 1 should contain 'Hello'"
    );

    let client2_output = result.get(&ClientId::new(2)).unwrap();
    assert!(
        client2_output.contains("World"),
        "Client 2 should contain 'World'"
    );
}

#[test]
fn every_client_gets_the_chunks_added_for_multiple_clients() {
    // The last client takes the chunks rather than a clone, which must not change what
    // any client receives - including when there is only one.
    for client_count in [1, 3] {
        let mut output = create_test_output();
        let client_ids = create_test_clients(client_count);
        let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
        output.add_clients(&client_ids, link_handler, None);

        let chunk = create_character_chunk_from_str("Shared", 0, 0);
        output
            .add_character_chunks_to_multiple_clients(vec![chunk], client_ids.iter().copied(), None)
            .unwrap();

        let result = output.serialize().unwrap();
        assert_eq!(result.len(), client_count);
        for client_id in &client_ids {
            assert!(
                result.get(client_id).unwrap().contains("Shared"),
                "client {client_id:?} of {client_count}"
            );
        }
    }
}

#[test]
fn test_serialize_with_pre_and_post_vte_instructions() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    output.add_pre_vte_instruction_to_client(ClientId::new(1), "\u{1b}[?1049h");
    let chunk = create_character_chunk_from_str("Test", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();
    output.add_post_vte_instruction_to_client(ClientId::new(1), "\u{1b}[?25h");

    let sized_result = output
        .clone()
        .serialize_with_size(Some(Size { rows: 1, cols: 4 }))
        .unwrap();
    let result = output.serialize().unwrap();
    assert_eq!(result, sized_result);
    let client_output = result.get(&ClientId::new(1)).unwrap();

    // Verify correct ordering
    let pre_vte_pos = client_output.find("\u{1b}[?1049h").unwrap();
    let text_pos = client_output.find("Test").unwrap();
    let post_vte_pos = client_output.find("\u{1b}[?25h").unwrap();

    assert!(
        pre_vte_pos < text_pos && text_pos < post_vte_pos,
        "Instructions should be in correct order: pre, content, post"
    );
}

#[test]
fn test_serialize_drains_state() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk = create_character_chunk_from_str("Hello", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    assert!(output.is_dirty(), "Output should be dirty before serialize");

    let result1 = output.serialize().unwrap();
    assert_eq!(result1.len(), 1, "First serialize should return data");

    assert!(
        !output.is_dirty(),
        "Output should not be dirty after serialize"
    );

    let result2 = output.serialize().unwrap();
    assert!(
        result2.is_empty(),
        "Second serialize should return empty HashMap"
    );
}

#[test]
fn test_serialize_with_size_no_constraints() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk = create_character_chunk_from_str("Hello", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    let result = output.serialize_with_size(None).unwrap();
    assert_eq!(result.len(), 1, "Should have one client in result");

    let client_output = result.get(&ClientId::new(1)).unwrap();
    assert!(client_output.contains("Hello"), "Should contain the text");
}

#[test]
fn test_serialize_with_size_crops_chunks_below_visible_area() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let max_size = Some(Size { rows: 10, cols: 80 });

    // Add chunk below visible area (should be cropped)
    let chunk_below = create_character_chunk_from_str("Hidden", 0, 15);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk_below], None)
        .unwrap();

    // Add chunk within visible area (should be included)
    let chunk_visible = create_character_chunk_from_str("Visible", 0, 5);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk_visible], None)
        .unwrap();

    let result = output.serialize_with_size(max_size).unwrap();
    let client_output = result.get(&ClientId::new(1)).unwrap();

    assert!(
        client_output.contains("Visible"),
        "Should contain visible chunk"
    );
    assert!(
        !client_output.contains("Hidden"),
        "Should not contain chunk below visible area"
    );
}

#[test]
fn test_serialize_with_size_crops_chunks_outside_cols() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let max_size = Some(Size { rows: 10, cols: 20 });

    // Add chunk outside visible columns (should be cropped)
    let chunk_outside = create_character_chunk_from_str("Hidden", 25, 5);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk_outside], None)
        .unwrap();

    // Add chunk within visible area
    let chunk_visible = create_character_chunk_from_str("Visible", 5, 5);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk_visible], None)
        .unwrap();

    let result = output.serialize_with_size(max_size).unwrap();
    let client_output = result.get(&ClientId::new(1)).unwrap();

    assert!(
        client_output.contains("Visible"),
        "Should contain visible chunk"
    );
    assert!(
        !client_output.contains("Hidden"),
        "Should not contain chunk outside visible columns"
    );
}

#[test]
fn test_serialize_with_size_crops_characters_within_chunk() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let max_size = Some(Size { rows: 10, cols: 20 });

    // Add chunk that starts at x=15 and would extend to x=25 (10 chars)
    let chunk = create_character_chunk_from_str("1234567890", 15, 5);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    let result = output.serialize_with_size(max_size).unwrap();
    let client_output = result.get(&ClientId::new(1)).unwrap();

    // Should only render first 5 characters (cols 15-19)
    assert!(
        client_output.contains("12345"),
        "Should contain first 5 characters"
    );
    assert!(
        !client_output.contains("67890"),
        "Should not contain characters beyond max_size.cols"
    );
}

#[test]
fn test_serialize_with_size_adds_no_padding() {
    for watcher_size in [
        Size { rows: 1, cols: 6 },
        Size { rows: 3, cols: 2 },
        Size { rows: 3, cols: 6 },
        Size { rows: 2, cols: 4 },
    ] {
        let mut output = create_test_output();
        let client = ClientId::new(1);
        output.add_clients(
            &create_test_clients(1),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output.add_pre_vte_instruction_to_client(client, "\x1b[?25l");
        output
            .add_character_chunks_to_client(
                client,
                vec![create_character_chunk_from_str("Hi", 0, 0)],
                None,
            )
            .unwrap();
        output.add_post_vte_instruction_to_client(client, "\x1b[6 q");
        let result = output.serialize_with_size(Some(watcher_size)).unwrap();
        assert_eq!(
            result[&client], "\x1b[?25l\x1b[1;1H\x1b[mHi\x1b[6 q",
            "watcher size: {watcher_size:?}"
        );
    }
}

#[test]
fn test_serialize_with_size_hides_cursor_when_cropped() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let max_size = Some(Size { rows: 10, cols: 20 });

    // Set cursor outside max_size
    output.set_cursor_coordinates(ClientId::new(1), 25, 5);

    let chunk = create_character_chunk_from_str("Test", 0, 0);
    output
        .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
        .unwrap();

    let result = output.serialize_with_size(max_size).unwrap();
    let client_output = result.get(&ClientId::new(1)).unwrap();

    // Verify hide cursor instruction is added
    assert!(
        client_output.contains("\u{1b}[?25l"),
        "Should contain hide cursor instruction when cursor is cropped"
    );
}

#[test]
fn test_serialize_with_size_crops_each_client_against_its_own_cursor() {
    // gp07: cursor coordinates are per client - a client whose own cursor fits must keep it, no
    // matter which other client rendered its (cropped) cursor into the same Output
    for reversed in [false, true] {
        let mut output = create_test_output();
        let client_ids = create_test_clients(2);
        let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
        output.add_clients(&client_ids, link_handler, None);

        let cursors = [(1, 25, 5), (2, 5, 5)];
        for (client_id, x, y) in if reversed {
            [cursors[1], cursors[0]]
        } else {
            cursors
        } {
            output.set_cursor_coordinates(ClientId::new(client_id), x, y);
        }

        for client_id in [1, 2] {
            output
                .add_character_chunks_to_client(
                    ClientId::new(client_id),
                    vec![create_character_chunk_from_str("Test", 0, 0)],
                    None,
                )
                .unwrap();
        }

        let result = output
            .serialize_with_size(Some(Size { rows: 10, cols: 20 }))
            .unwrap();
        assert!(
            result
                .get(&ClientId::new(1))
                .unwrap()
                .contains("\u{1b}[?25l"),
            "client 1's cursor is outside its size and must be hidden (reversed: {reversed})"
        );
        assert!(
            !result
                .get(&ClientId::new(2))
                .unwrap()
                .contains("\u{1b}[?25l"),
            "client 2's cursor fits and must not be hidden by client 1's (reversed: {reversed})"
        );
    }
}

#[test]
fn test_add_character_chunks_to_multiple_clients() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(3);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let chunk = create_character_chunk_from_str("Test", 0, 0);
    output
        .add_character_chunks_to_multiple_clients(vec![chunk], client_ids.iter().copied(), None)
        .unwrap();

    let result = output.serialize().unwrap();
    assert_eq!(result.len(), 3, "Should have three clients in result");

    for client_id in 1..=3 {
        let client_output = result.get(&ClientId::new(client_id)).unwrap();
        assert!(
            client_output.contains("Test"),
            "Client {} should contain the text",
            client_id
        );
    }
}

#[test]
fn test_add_sixel_image_chunks_to_multiple_clients() {
    let mut output = create_test_output();
    let client_ids = create_test_clients(2);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);

    let sixel_chunk = SixelImageChunk {
        cell_x: 0,
        cell_y: 0,
        sixel_image_pixel_x: 0,
        sixel_image_pixel_y: 0,
        sixel_image_pixel_width: 100,
        sixel_image_pixel_height: 100,
        sixel_image_id: 1,
    };

    output.add_sixel_image_chunks_to_multiple_clients(
        vec![sixel_chunk],
        client_ids.iter().copied(),
        None,
    );

    assert!(
        output.has_rendered_assets(),
        "Output should have rendered assets"
    );
}

#[test]
fn test_character_chunk_new() {
    let terminal_chars: Vec<TerminalCharacter> = vec![TerminalCharacter::new('A')];

    let chunk = CharacterChunk::new(terminal_chars, 5, 10);

    assert_eq!(chunk.x, 5, "x should be set correctly");
    assert_eq!(chunk.y, 10, "y should be set correctly");
    assert_eq!(
        chunk.terminal_characters.len(),
        1,
        "Should have one character"
    );
}

mod character_chunk_props {
    use super::*;
    use hegel::generators;
    use unicode_width::UnicodeWidthChar;

    #[hegel::composite]
    fn chunks(tc: &hegel::TestCase) -> CharacterChunk {
        // Full unicode: wide CJK, combining marks and zero-width characters are
        // exactly where the width bookkeeping goes wrong.
        let text = tc.draw(generators::text().max_size(40));
        let x = tc.draw(generators::integers::<usize>().max_value(500));
        let y = tc.draw(generators::integers::<usize>().max_value(500));
        create_character_chunk_from_str(&text, x, y)
    }

    /// Replaces `test_character_chunk_width`, which asserted this for "Hello"
    /// and for one mixed-width string.
    #[hegel::test]
    fn width_is_the_sum_of_the_character_widths(tc: hegel::TestCase) {
        let chunk = tc.draw(chunks());
        let expected: usize = chunk
            .terminal_characters
            .iter()
            .map(|c| c.character.width().unwrap_or(0))
            .sum();
        assert_eq!(
            chunk.width(),
            expected,
            "width() disagreed with unicode-width for {:?}",
            chunk.terminal_characters
        );
    }

    /// Replaces `test_character_chunk_drain_by_width` and
    /// `..._with_wide_chars`. Splitting a wide character substitutes blanks on
    /// both sides, so the characters are not preserved - but the total width
    /// always is, and that is the invariant the renderer depends on.
    #[hegel::test]
    fn draining_conserves_total_width(tc: hegel::TestCase) {
        let mut chunk = tc.draw(chunks());
        let original_width = chunk.width();
        let x = tc.draw(generators::integers::<usize>().max_value(original_width + 5));

        let drained: Vec<TerminalCharacter> = chunk.drain_by_width(x).collect();
        let drained_width: usize = drained.iter().map(|c| c.width()).sum();

        assert_eq!(
            drained_width + chunk.width(),
            original_width,
            "draining {} lost or invented width: {} drained + {} left != {}",
            x,
            drained_width,
            chunk.width(),
            original_width
        );
        assert_eq!(drained_width, x.min(original_width));
    }

    /// Retaining must fill the budget when enough text exists: dropping all
    /// text also fits the budget, but would erase uncovered cells.
    #[hegel::test]
    fn retaining_never_leaves_more_than_the_budget(tc: hegel::TestCase) {
        let mut chunk = tc.draw(chunks());
        let original_width = chunk.width();
        let x = tc.draw(generators::integers::<usize>().max_value(original_width + 5));

        chunk.retain_by_width(x);
        assert_eq!(chunk.width(), x.min(original_width));
    }

    /// Replaces `test_character_chunk_cut_middle_out`, which cut one hardcoded
    /// span out of one ASCII string. A floating pane can occlude any span.
    #[hegel::test]
    fn cutting_the_middle_out_never_panics_and_keeps_the_edges(tc: hegel::TestCase) {
        let mut chunk = tc.draw(chunks());
        let width = chunk.width();
        let a = tc.draw(generators::integers::<usize>().max_value(width + 2));
        let b = tc.draw(generators::integers::<usize>().max_value(width + 2));
        let (middle_start, middle_end) = if a <= b { (a, b) } else { (b, a) };

        let Ok((left, right)) = chunk.cut_middle_out(middle_start, middle_end) else {
            // Out-of-range spans are reported, not panicked on - that is the
            // property for those inputs.
            assert!(middle_end > width, "a valid cut must succeed");
            return;
        };
        let left_width: usize = left.iter().map(|c| c.width()).sum();
        let right_width: usize = right.iter().map(|c| c.width()).sum();
        // The left piece is rendered at the chunk's own x, so it may occupy at
        // most the columns before the cut; likewise the right piece may occupy
        // at most the columns after it. Anything more overdraws the pane that
        // the cut was making room for.
        assert_eq!(
            left_width, middle_start,
            "cutting {}..{} out of a {}-wide chunk left a {}-wide left piece, \
             which does not end exactly where the cut begins",
            middle_start, middle_end, width, left_width
        );
        assert_eq!(
            right_width,
            width.saturating_sub(middle_end),
            "cutting {}..{} out of a {}-wide chunk left a {}-wide right piece",
            middle_start,
            middle_end,
            width,
            right_width
        );
    }

    #[test]
    fn clipping_keeps_text_and_blanks_only_the_split_wide_character() {
        let text = "a界b語c";
        let prefixes = ["", "a", "a ", "a界", "a界b", "a界b ", "a界b語", text];
        let suffixes = [text, "界b語c", " b語c", "b語c", "語c", " c", "c", ""];
        let as_text =
            |chars: &[TerminalCharacter]| chars.iter().map(|c| c.character).collect::<String>();

        for cut in 0..=9 {
            let mut chunk = create_character_chunk_from_str(text, 0, 0);
            let drained: Vec<_> = chunk.drain_by_width(cut).collect();
            assert_eq!(as_text(&drained), prefixes[cut.min(7)], "drain at {cut}");
            assert_eq!(as_text(&chunk.terminal_characters), suffixes[cut.min(7)]);

            let mut chunk = create_character_chunk_from_str(text, 0, 0);
            chunk.retain_by_width(cut);
            assert_eq!(as_text(&chunk.terminal_characters), prefixes[cut.min(7)]);
        }
        for (start, prefix) in prefixes.iter().enumerate() {
            for (end, suffix) in suffixes.iter().enumerate().skip(start) {
                let mut chunk = create_character_chunk_from_str(text, 0, 0);
                let (left, right) = chunk.cut_middle_out(start, end).unwrap();
                assert_eq!(as_text(&left), *prefix, "left of {start}..{end}");
                assert_eq!(as_text(&right), *suffix, "right of {start}..{end}");
            }
        }
    }

    #[hegel::test]
    fn occlusion_preserves_exactly_the_uncovered_styled_cells(tc: hegel::TestCase) {
        use crate::panes::terminal_character::DEFAULT_STYLES;
        use std::collections::BTreeMap;

        let mut chunks = Vec::new();
        let mut source_cells = BTreeMap::new();
        for y in 0..3 {
            let x = tc.draw(generators::integers::<usize>().max_value(8));
            let length = tc.draw(generators::integers::<usize>().min_value(1).max_value(16));
            let characters: Vec<_> = (0..length)
                .map(|i| {
                    let character = TerminalCharacter::new_styled(
                        char::from(b'a' + i as u8),
                        DEFAULT_STYLES
                            .foreground(Some(AnsiCode::ColorIndex((i + y * 16) as u8)))
                            .into(),
                    );
                    source_cells.insert((x + i, y), character.clone());
                    character
                })
                .collect();
            chunks.push(CharacterChunk::new(characters, x, y));
        }
        let layer_count = tc.draw(generators::integers::<usize>().max_value(5));
        let layers = (0..layer_count)
            .map(|_| {
                create_pane_geom(
                    tc.draw(generators::integers::<usize>().max_value(24)),
                    tc.draw(generators::integers::<usize>().max_value(3)),
                    tc.draw(generators::integers::<usize>().min_value(1).max_value(16)),
                    tc.draw(generators::integers::<usize>().min_value(1).max_value(3)),
                )
            })
            .collect();
        let stack = FloatingPanesStack { layers };

        // The oracle removes covered cells directly; it never splits a chunk or
        // calls the clipping helpers used by the renderer.
        for z_index in std::iter::once(None).chain((0..=layer_count).map(Some)) {
            let mut expected = source_cells.clone();
            expected.retain(|&(x, y), _| {
                !stack.layers.iter().enumerate().any(|(layer, pane)| {
                    layer >= z_index.unwrap_or(0)
                        && (pane.x..pane.x + pane.cols.as_usize()).contains(&x)
                        && (pane.y..pane.y + pane.rows.as_usize()).contains(&y)
                })
            });
            let visible = stack
                .visible_character_chunks(chunks.clone(), z_index)
                .unwrap();
            let mut actual = BTreeMap::new();
            for chunk in visible {
                for (offset, character) in chunk.terminal_characters.into_iter().enumerate() {
                    let position = (chunk.x + offset, chunk.y);
                    assert!(
                        actual.insert(position, character).is_none(),
                        "duplicate cell {position:?}"
                    );
                }
            }
            assert_eq!(actual, expected, "z-index {z_index:?}, stack {stack:?}");
        }
    }
}

#[test]
fn test_visible_character_chunks_no_panes() {
    let stack = FloatingPanesStack { layers: vec![] };
    let chunks = vec![create_character_chunk_from_str("Test", 0, 0)];

    let visible = stack.visible_character_chunks(chunks, None).unwrap();

    assert_eq!(visible.len(), 1, "All chunks should be visible");
}

#[test]
fn test_visible_character_chunks_completely_covered() {
    let pane_geom = create_pane_geom(0, 0, 10, 10);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    // Chunk completely within pane bounds
    let chunks = vec![create_character_chunk_from_str("Test", 5, 5)];

    let visible = stack.visible_character_chunks(chunks, Some(0)).unwrap();

    assert_eq!(
        visible.len(),
        0,
        "Completely covered chunk should not be visible"
    );
}

#[test]
fn test_visible_character_chunks_partially_covered_left() {
    let pane_geom = create_pane_geom(0, 5, 10, 1);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    // Chunk that spans x=5-15, pane covers x=0-9
    let chunks = vec![create_character_chunk_from_str("HelloWorld", 5, 5)];

    let visible = stack.visible_character_chunks(chunks, Some(0)).unwrap();

    // Should retain the right part
    assert!(!visible.is_empty(), "Should have visible chunks");
    if !visible.is_empty() {
        assert!(
            visible[0].x >= 10,
            "Visible chunk should start after pane edge"
        );
    }
}

#[test]
fn test_visible_character_chunks_partially_covered_right() {
    let pane_geom = create_pane_geom(10, 5, 10, 1);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    // Chunk that spans x=5-15, pane covers x=10-19
    let chunks = vec![create_character_chunk_from_str("HelloWorld", 5, 5)];

    let visible = stack.visible_character_chunks(chunks, Some(0)).unwrap();

    // Should retain the left part
    assert!(!visible.is_empty(), "Should have visible chunks");
    if !visible.is_empty() {
        assert_eq!(visible[0].x, 5, "Visible chunk should start at original x");
        assert!(
            visible[0].width() < 10,
            "Visible chunk should be shorter than original"
        );
    }
}

#[test]
fn test_visible_character_chunks_pane_beside_chunk() {
    // gp15: the "covers the chunk partially to the right" branch must not also match a pane that
    // sits entirely beside the chunk - it is bounded by the chunk's right side, mirroring the
    // left-partial branch
    let chunk_spans_5_to_14 = || vec![create_character_chunk_from_str("HelloWorld", 5, 5)];

    // pane at x=20-29: no horizontal overlap at all, the chunk is untouched
    let stack = FloatingPanesStack {
        layers: vec![create_pane_geom(20, 5, 10, 1)],
    };
    let visible = stack
        .visible_character_chunks(chunk_spans_5_to_14(), Some(0))
        .unwrap();
    assert_eq!(visible.len(), 1, "a pane beside the chunk covers nothing");
    assert_eq!(visible[0].x, 5);
    assert_eq!(visible[0].width(), 10, "the whole chunk stays visible");

    // pane at x=14-23: overlaps the chunk's last column only
    let stack = FloatingPanesStack {
        layers: vec![create_pane_geom(14, 5, 10, 1)],
    };
    let visible = stack
        .visible_character_chunks(chunk_spans_5_to_14(), Some(0))
        .unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].x, 5);
    assert_eq!(visible[0].width(), 9, "only the covered last column is cut");
}

#[test]
fn test_visible_character_chunks_middle_covered() {
    let pane_geom = create_pane_geom(5, 5, 3, 1);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    // Chunk spans x=0-10, pane covers x=5-7
    let chunks = vec![create_character_chunk_from_str("0123456789", 0, 5)];

    let visible = stack.visible_character_chunks(chunks, Some(0)).unwrap();

    // Should return two chunks (left and right parts)
    assert!(
        !visible.is_empty(),
        "Should have at least one visible chunk when middle is covered"
    );
}

#[test]
fn test_cursor_is_visible_with_floating_panes() {
    let pane_geom = create_pane_geom(5, 5, 10, 10);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    // Cursor inside pane bounds
    assert!(
        !stack.cursor_is_visible(7, 7, None),
        "Cursor should not be visible when covered by pane"
    );

    // Cursor outside pane bounds
    assert!(
        stack.cursor_is_visible(20, 20, None),
        "Cursor should be visible when not covered by pane"
    );
}

/// A context menu is drawn above every pane and every toast, so nothing may put a cursor - the
/// client's own or the fake one standing in for someone else - on a cell it covers. Unlike the
/// toasts, a menu belongs to one client: another client is looking at the same cells with nothing
/// drawn over them.
#[test]
fn test_cursor_is_not_visible_under_a_context_menu() {
    use crate::ui::damage::CellRect;

    let mut output = create_test_output();
    output.set_context_menu_rects(HashMap::from([(
        ClientId::new(1),
        vec![CellRect {
            x: 5,
            y: 5,
            cols: 10,
            rows: 4,
        }],
    )]));

    assert!(
        !output.cursor_is_visible(7, 6, None, ClientId::new(1)),
        "the menu's own client cannot be shown a cursor under it"
    );
    assert!(
        output.cursor_is_visible(7, 9, None, ClientId::new(1)),
        "one row below the menu is not covered"
    );
    assert!(
        output.cursor_is_visible(15, 6, None, ClientId::new(1)),
        "one column past the menu is not covered"
    );
    assert!(
        output.cursor_is_visible(7, 6, None, ClientId::new(2)),
        "a client without this menu sees those cells uncovered"
    );
}

#[test]
fn test_cursor_visibility_with_z_index_bottom_layer() {
    // Two overlapping panes: bottom at (5,5) and top at (5,5)
    let bottom_pane = create_pane_geom(5, 5, 10, 10);
    let top_pane = create_pane_geom(5, 5, 10, 10);

    let stack = FloatingPanesStack {
        layers: vec![bottom_pane, top_pane],
    };

    // Cursor at position (7,7) in the bottom layer (z-index 0)
    // Should be hidden because there's a pane above it (z-index 1) at the same position
    assert!(
        !stack.cursor_is_visible(7, 7, Some(0)),
        "Cursor in bottom layer should be hidden by pane above it"
    );
}

#[test]
fn test_cursor_visibility_with_z_index_top_layer() {
    // Two overlapping panes: bottom at (5,5) and top at (5,5)
    let bottom_pane = create_pane_geom(5, 5, 10, 10);
    let top_pane = create_pane_geom(5, 5, 10, 10);

    let stack = FloatingPanesStack {
        layers: vec![bottom_pane, top_pane],
    };

    // Cursor at position (7,7) in the top layer (z-index 1)
    // Should be visible even though there's a pane below it at the same position
    assert!(
        stack.cursor_is_visible(7, 7, Some(1)),
        "Cursor in top layer should be visible (not affected by panes below)"
    );
}

#[test]
fn test_cursor_visibility_with_multiple_layers() {
    // Three panes stacked vertically with different layers
    // Layer 0 (bottom): pane at (5,5)
    // Layer 1 (middle): pane at (10,10)
    // Layer 2 (top): pane at (5,5) - overlaps with layer 0
    let layer0_pane = create_pane_geom(5, 5, 10, 10);
    let layer1_pane = create_pane_geom(10, 10, 10, 10);
    let layer2_pane = create_pane_geom(5, 5, 10, 10);

    let stack = FloatingPanesStack {
        layers: vec![layer0_pane, layer1_pane, layer2_pane],
    };

    // Cursor at (7,7) in layer 0 - should be hidden by layer 2
    assert!(
        !stack.cursor_is_visible(7, 7, Some(0)),
        "Cursor in layer 0 should be hidden by layer 2 above it"
    );

    // Cursor at (16,16) in layer 1 - should be visible (layer 2 doesn't cover this position)
    // Layer 1 is at (10,10) size 10x10, so covers (10,10) to (19,19)
    // Layer 2 is at (5,5) size 10x10, so covers (5,5) to (14,14)
    // Position (16,16) is inside layer 1 but outside layer 2
    assert!(
        stack.cursor_is_visible(16, 16, Some(1)),
        "Cursor in layer 1 should be visible when not covered by layers above"
    );

    // Cursor at (7,7) in layer 2 - should be visible (no layers above)
    assert!(
        stack.cursor_is_visible(7, 7, Some(2)),
        "Cursor in top layer should always be visible"
    );
}

#[test]
fn test_cursor_visibility_pinned_pane_over_floating() {
    // Simulates a floating pane (z-index 0) with a pinned pane (z-index 1) on top
    // Both panes overlap at the same position
    let floating_pane = create_pane_geom(10, 10, 20, 20);
    let mut pinned_pane = create_pane_geom(10, 10, 20, 20);
    pinned_pane.is_pinned = true;

    let stack = FloatingPanesStack {
        layers: vec![floating_pane, pinned_pane],
    };

    // Cursor in the floating pane (z-index 0) at position covered by pinned pane
    assert!(
        !stack.cursor_is_visible(15, 15, Some(0)),
        "Cursor in floating pane should be hidden when covered by pinned pane above"
    );

    // Cursor in the pinned pane (z-index 1) at the same position
    assert!(
        stack.cursor_is_visible(15, 15, Some(1)),
        "Cursor in pinned pane should be visible"
    );
}

#[test]
fn test_cursor_visibility_partial_overlap() {
    // Two panes with partial overlap
    // Layer 0: pane at (0, 0) size 10x10
    // Layer 1: pane at (5, 5) size 10x10 (overlaps bottom-right of layer 0)
    let layer0_pane = create_pane_geom(0, 0, 10, 10);
    let layer1_pane = create_pane_geom(5, 5, 10, 10);

    let stack = FloatingPanesStack {
        layers: vec![layer0_pane, layer1_pane],
    };

    // Cursor at (2,2) in layer 0 - not covered by layer 1, should be visible
    assert!(
        stack.cursor_is_visible(2, 2, Some(0)),
        "Cursor in non-overlapping area should be visible"
    );

    // Cursor at (7,7) in layer 0 - covered by layer 1, should be hidden
    assert!(
        !stack.cursor_is_visible(7, 7, Some(0)),
        "Cursor in overlapping area should be hidden by pane above"
    );

    // Cursor at (7,7) in layer 1 - should be visible (top layer)
    assert!(
        stack.cursor_is_visible(7, 7, Some(1)),
        "Cursor in top layer should be visible in overlapping area"
    );
}

#[test]
fn test_output_buffer_update_line() {
    let mut buffer = OutputBuffer::default();
    buffer.clear(); // Clear the initial "update all lines" state

    buffer.update_line(5);

    assert!(
        buffer.changed_lines.contains(5),
        "Changed lines should contain line 5"
    );
}

#[test]
fn test_output_buffer_update_all_lines() {
    let mut buffer = OutputBuffer::default();

    assert!(
        buffer.should_update_all_lines,
        "Should update all lines by default"
    );

    buffer.clear();
    assert!(
        !buffer.should_update_all_lines,
        "Should not update all lines after clear"
    );

    buffer.update_all_lines();
    assert!(
        buffer.should_update_all_lines,
        "Should update all lines after update_all_lines"
    );
}

#[test]
fn test_output_buffer_serialize() {
    let buffer = OutputBuffer::default();

    // Create a simple viewport with Row data
    let mut columns = VecDeque::new();
    columns.push_back(TerminalCharacter::new('A'));
    let row = Row::from_columns(columns);
    let viewport = vec![row];

    let result = buffer.serialize(&viewport, true, None).unwrap();

    // Should contain the character and newlines/carriage returns
    assert!(result.contains('A'), "Serialized output should contain 'A'");
    assert!(
        result.contains("\n\r"),
        "Serialized output should contain newlines"
    );
}

#[test]
fn test_output_buffer_changed_chunks_in_viewport_when_all_dirty() {
    let buffer = OutputBuffer::default();

    let mut columns = VecDeque::new();
    columns.push_back(TerminalCharacter::new('A'));
    let row = Row::from_columns(columns);
    let viewport = vec![row];

    let chunks = buffer.changed_chunks_in_viewport(&viewport, 10, 1, 0, 0);

    assert_eq!(
        chunks.len(),
        1,
        "Should return all lines when should_update_all_lines is true"
    );
}

#[test]
fn test_output_buffer_changed_chunks_in_viewport_partial() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    // Mark only specific lines as changed
    buffer.update_line(2);
    buffer.update_line(5);
    buffer.update_line(7);

    let rows: Vec<Row> = (0..10)
        .map(|_| {
            let mut columns = VecDeque::new();
            columns.push_back(TerminalCharacter::new('A'));
            Row::from_columns(columns)
        })
        .collect();

    let chunks = buffer.changed_chunks_in_viewport(&rows, 10, 10, 0, 0);

    assert_eq!(chunks.len(), 3, "Should return only changed lines");
    assert_eq!(chunks[0].y, 2, "First chunk should be at line 2");
    assert_eq!(chunks[1].y, 5, "Second chunk should be at line 5");
    assert_eq!(chunks[2].y, 7, "Third chunk should be at line 7");
}

fn viewport_of(line_count: usize) -> Vec<Row> {
    (0..line_count)
        .map(|_| {
            let mut columns = VecDeque::new();
            columns.push_back(TerminalCharacter::new('A'));
            Row::from_columns(columns)
        })
        .collect()
}

#[test]
fn changed_lines_tracks_indices_across_word_boundaries() {
    let mut changed_lines = ChangedLines::default();
    // the bitset stores 64 indices per word, so these straddle the first three words
    let marked = [0, 1, 63, 64, 65, 127, 128, 1000];
    for line_index in marked {
        changed_lines.insert(line_index);
    }

    for line_index in marked {
        assert!(
            changed_lines.contains(line_index),
            "line {} should be marked",
            line_index
        );
    }
    for line_index in [2, 62, 66, 126, 129, 999, 1001] {
        assert!(
            !changed_lines.contains(line_index),
            "line {} should not be marked",
            line_index
        );
    }
    // beyond any word this set ever allocated
    assert!(!changed_lines.contains(100_000));
}

#[test]
fn changed_lines_iterates_in_ascending_order() {
    let mut changed_lines = ChangedLines::default();
    // inserted descending and out of order, spanning several words
    for line_index in [200, 64, 199, 5, 63, 128, 0] {
        changed_lines.insert(line_index);
    }

    let collected: Vec<usize> = changed_lines.iter().collect();
    assert_eq!(
        collected,
        vec![0, 5, 63, 64, 128, 199, 200],
        "iteration should be ascending, each index exactly once"
    );
}

#[test]
fn changed_lines_inserting_the_same_index_twice_yields_it_once() {
    let mut changed_lines = ChangedLines::default();
    changed_lines.insert(7);
    changed_lines.insert(7);
    changed_lines.insert(7);

    assert_eq!(changed_lines.iter().collect::<Vec<_>>(), vec![7]);
}

#[test]
fn changed_lines_clear_empties_the_set_and_stays_usable() {
    let mut changed_lines = ChangedLines::default();
    assert!(changed_lines.is_empty(), "a fresh set is empty");

    changed_lines.insert(3);
    changed_lines.insert(300);
    assert!(!changed_lines.is_empty());

    changed_lines.clear();
    assert!(changed_lines.is_empty(), "cleared set should be empty");
    assert_eq!(changed_lines.iter().count(), 0);
    assert!(!changed_lines.contains(3));
    assert!(!changed_lines.contains(300));

    // the allocation is kept across a clear, so re-inserting must still work
    changed_lines.insert(300);
    assert!(!changed_lines.is_empty());
    assert_eq!(changed_lines.iter().collect::<Vec<_>>(), vec![300]);
}

#[test]
fn test_output_buffer_update_lines_marks_inclusive_range() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    buffer.update_lines(3, 6);

    assert_eq!(
        buffer.changed_lines.iter().collect::<Vec<_>>(),
        vec![3, 4, 5, 6],
        "update_lines should mark the range inclusively"
    );
}

#[test]
fn test_output_buffer_update_lines_is_a_noop_when_all_lines_are_dirty() {
    let mut buffer = OutputBuffer::default();
    assert!(buffer.should_update_all_lines);

    buffer.update_lines(3, 6);

    assert!(
        buffer.changed_lines.is_empty(),
        "there is no point tracking individual lines when everything is dirty"
    );
}

#[test]
fn test_changed_rects_groups_a_contiguous_run_into_one_rect() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    // marked out of order on purpose: grouping depends on ascending iteration
    buffer.update_line(5);
    buffer.update_line(3);
    buffer.update_line(4);

    let changed_rects = buffer.changed_rects_in_viewport(10);

    let mut expected = HashMap::new();
    expected.insert(3, 3);
    assert_eq!(
        changed_rects, expected,
        "lines 3, 4 and 5 are contiguous and should form a single rect"
    );
}

#[test]
fn test_changed_rects_keeps_separate_runs_apart() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    for line_index in [8, 1, 7, 2] {
        buffer.update_line(line_index);
    }

    let changed_rects = buffer.changed_rects_in_viewport(10);

    let mut expected = HashMap::new();
    expected.insert(1, 2);
    expected.insert(7, 2);
    assert_eq!(changed_rects, expected);
}

#[test]
fn test_changed_rects_when_all_lines_are_dirty() {
    let buffer = OutputBuffer::default();
    assert!(buffer.should_update_all_lines);

    let changed_rects = buffer.changed_rects_in_viewport(10);

    let mut expected = HashMap::new();
    expected.insert(0, 10);
    assert_eq!(changed_rects, expected);
}

#[test]
fn test_changed_rects_across_bitset_word_boundaries() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    // a run that crosses the 64-index word boundary of the underlying bitset
    buffer.update_lines(62, 66);

    let changed_rects = buffer.changed_rects_in_viewport(100);

    let mut expected = HashMap::new();
    expected.insert(62, 5);
    assert_eq!(
        changed_rects, expected,
        "a run spanning two bitset words is still one rect"
    );
}

#[test]
fn test_output_buffer_changed_chunks_are_ascending_regardless_of_mark_order() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    for line_index in [7, 2, 9, 0, 5] {
        buffer.update_line(line_index);
    }

    let chunks = buffer.changed_chunks_in_viewport(&viewport_of(10), 10, 10, 0, 0);

    assert_eq!(
        chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
        vec![0, 2, 5, 7, 9]
    );
}

#[test]
fn test_output_buffer_changed_chunks_drops_lines_below_the_viewport() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    buffer.update_line(1);
    buffer.update_line(3);
    // the viewport shrank since these were marked
    buffer.update_line(5);
    buffer.update_line(70);

    let chunks = buffer.changed_chunks_in_viewport(&viewport_of(4), 10, 4, 0, 0);

    assert_eq!(
        chunks.iter().map(|chunk| chunk.y).collect::<Vec<_>>(),
        vec![1, 3],
        "lines at or past viewport_height should be dropped"
    );
}

#[test]
fn test_output_buffer_changed_chunks_apply_offsets() {
    let mut buffer = OutputBuffer::default();
    buffer.clear();

    buffer.update_line(1);
    buffer.update_line(2);

    let chunks = buffer.changed_chunks_in_viewport(&viewport_of(10), 10, 10, 4, 20);

    assert_eq!(
        chunks
            .iter()
            .map(|chunk| (chunk.x, chunk.y))
            .collect::<Vec<_>>(),
        vec![(4, 21), (4, 22)]
    );
}

#[test]
fn test_pane_defaults_preserved_when_middle_covered() {
    let pane_geom = create_pane_geom(5, 5, 3, 1);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    let pane_bg = Some(AnsiCode::RgbCode((0, 26, 58)));
    let pane_fg = Some(AnsiCode::RgbCode((0, 224, 0)));

    let mut chunk = create_character_chunk_from_str("0123456789", 0, 5);
    chunk.pane_default_bg = pane_bg;
    chunk.pane_default_fg = pane_fg;

    let visible = stack
        .visible_character_chunks(vec![chunk], Some(0))
        .unwrap();

    assert_eq!(visible.len(), 2, "Middle split should produce two chunks");
    for (i, chunk) in visible.iter().enumerate() {
        assert_eq!(
            chunk.pane_default_bg, pane_bg,
            "Chunk {i} should preserve pane_default_bg"
        );
        assert_eq!(
            chunk.pane_default_fg, pane_fg,
            "Chunk {i} should preserve pane_default_fg"
        );
    }
}

#[test]
fn test_pane_defaults_preserved_when_left_covered() {
    let pane_geom = create_pane_geom(0, 5, 5, 1);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    let pane_bg = Some(AnsiCode::RgbCode((0, 26, 58)));

    let mut chunk = create_character_chunk_from_str("0123456789", 0, 5);
    chunk.pane_default_bg = pane_bg;

    let visible = stack
        .visible_character_chunks(vec![chunk], Some(0))
        .unwrap();

    assert_eq!(visible.len(), 1, "Left-covered should produce one chunk");
    assert_eq!(
        visible[0].pane_default_bg, pane_bg,
        "Remaining chunk should preserve pane_default_bg"
    );
}

#[test]
fn test_pane_defaults_preserved_when_right_covered() {
    let pane_geom = create_pane_geom(5, 5, 10, 1);
    let stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };

    let pane_bg = Some(AnsiCode::RgbCode((0, 26, 58)));

    let mut chunk = create_character_chunk_from_str("0123456789", 0, 5);
    chunk.pane_default_bg = pane_bg;

    let visible = stack
        .visible_character_chunks(vec![chunk], Some(0))
        .unwrap();

    assert_eq!(visible.len(), 1, "Right-covered should produce one chunk");
    assert_eq!(
        visible[0].pane_default_bg, pane_bg,
        "Remaining chunk should preserve pane_default_bg"
    );
}

pub(super) type KittyTestParts = (
    Rc<RefCell<KittyImageStore>>,
    Rc<RefCell<HashMap<ClientId, bool>>>,
    Rc<RefCell<HashMap<ClientId, HostKittyState>>>,
);

pub(super) fn create_test_kitty_parts() -> KittyTestParts {
    let kitty_image_store = Rc::new(RefCell::new(KittyImageStore::default()));
    let capabilities = Rc::new(RefCell::new(HashMap::new()));
    capabilities.borrow_mut().insert(ClientId::new(1), true);
    let host_state = Rc::new(RefCell::new(HashMap::new()));
    (kitty_image_store, capabilities, host_state)
}

fn create_test_kitty_output(parts: &KittyTestParts) -> Output {
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        height: 20,
        width: 10,
    })));
    Output::new(
        sixel_image_store,
        character_cell_size,
        true,
        true,
        parts.0.clone(),
        parts.1.clone(),
        parts.2.clone(),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
    )
}

pub(super) fn store_test_kitty_image(
    kitty_image_store: &Rc<RefCell<KittyImageStore>>,
    width: u32,
    height: u32,
) -> InternalImageId {
    kitty_image_store
        .borrow_mut()
        .store_image(DecodedImage {
            bytes: {
                let mut state = 42u32;
                (0..width * height * 4)
                    .map(|_| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        state as u8
                    })
                    .collect()
            },
            width,
            height,
            format: KittyFormat::Rgba32,
        })
        .unwrap()
}

pub(super) fn kitty_chunk(
    internal_image_id: InternalImageId,
    placement_uid: u64,
    cell_x: usize,
    cell_y: usize,
) -> KittyImageChunk {
    KittyImageChunk {
        cell_x,
        cell_y,
        internal_image_id,
        source_px_x: 0,
        source_px_y: 0,
        source_px_width: 30,
        source_px_height: 40,
        cell_offset_x: 0,
        cell_offset_y: 0,
        z_index: 0,
        dest_cells: (3, 2),
        variant_key: crate::panes::kitty_graphics::store::ScaledImageKey {
            source: crate::panes::sixel::PixelRect::new(0, 0, 40, 30),
            size: (30, 40),
        },
        scaled_px: None,
        placement_uid,
    }
}

pub(super) fn run_kitty_frame(
    parts: &KittyTestParts,
    chunks: Vec<KittyImageChunk>,
    floating_panes_stack: Option<FloatingPanesStack>,
) -> String {
    run_kitty_frame_with_cell_size(
        parts,
        chunks,
        floating_panes_stack,
        SizeInPixels {
            width: 10,
            height: 20,
        },
    )
}

pub(super) fn run_kitty_frame_with_cell_size(
    parts: &KittyTestParts,
    chunks: Vec<KittyImageChunk>,
    floating_panes_stack: Option<FloatingPanesStack>,
    cell_size: SizeInPixels,
) -> String {
    let mut output = create_test_kitty_output(parts);
    *output.character_cell_size.borrow_mut() = Some(cell_size);
    let client_ids: HashSet<ClientId> = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, floating_panes_stack);
    output.add_kitty_image_chunks_to_client(
        ClientId::new(1),
        PaneId::Terminal(TerminalId::new(1)),
        chunks,
        None,
    );
    output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default()
}

fn run_kitty_frame_for_panes(
    parts: &KittyTestParts,
    chunks_by_pane: Vec<(PaneId, Vec<KittyImageChunk>)>,
    visible_panes: HashSet<PaneId>,
) -> String {
    let mut output = create_test_kitty_output(parts);
    let client_ids: HashSet<ClientId> = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);
    output.set_kitty_visible_panes(ClientId::new(1), visible_panes);
    for (pane_id, chunks) in chunks_by_pane {
        output.add_kitty_image_chunks_to_client(ClientId::new(1), pane_id, chunks, None);
    }
    output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default()
}

fn run_kitty_frame_with_host_clear(parts: &KittyTestParts, chunks: Vec<KittyImageChunk>) -> String {
    let mut output = create_test_kitty_output(parts);
    let client_ids: HashSet<ClientId> = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);
    output.add_pre_vte_instruction_to_client(ClientId::new(1), "\u{1b}[m\u{1b}[2J");
    output.mark_host_display_cleared_for_clients(std::iter::once(ClientId::new(1)));
    output.add_kitty_image_chunks_to_client(
        ClientId::new(1),
        PaneId::Terminal(TerminalId::new(1)),
        chunks,
        None,
    );
    output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default()
}

fn run_kitty_frame_with_unmarked_clear_string(
    parts: &KittyTestParts,
    chunks: Vec<KittyImageChunk>,
) -> String {
    let mut output = create_test_kitty_output(parts);
    let client_ids: HashSet<ClientId> = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);
    output.add_pre_vte_instruction_to_client(ClientId::new(1), "\u{1b}[m\u{1b}[2J");
    output.add_kitty_image_chunks_to_client(
        ClientId::new(1),
        PaneId::Terminal(TerminalId::new(1)),
        chunks,
        None,
    );
    output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap_or_default()
}

fn parse_kitty_placement_crops(output: &str) -> Vec<(usize, usize, usize, usize, usize, usize)> {
    let marker = "\u{1b}_Ga=p,q=2,";
    let mut crops = vec![];
    let mut search_start = 0;
    while let Some(position) = output[search_start..].find(marker) {
        let absolute_position = search_start + position;
        let after = &output[absolute_position + marker.len()..];
        let end = after.find("\u{1b}\\").unwrap();
        let mut params: HashMap<&str, &str> = HashMap::new();
        for key_value in after[..end].split(',') {
            let mut parts = key_value.splitn(2, '=');
            let key = parts.next().unwrap();
            let value = parts.next().unwrap_or("");
            params.insert(key, value);
        }
        let before = output[..absolute_position]
            .strip_suffix("\u{1b}[m")
            .unwrap();
        let goto_start = before.rfind("\u{1b}[").unwrap();
        let coordinates = &before[goto_start + 2..before.len() - 1];
        let mut coordinate_parts = coordinates.split(';');
        let row: usize = coordinate_parts.next().unwrap().parse().unwrap();
        let column: usize = coordinate_parts.next().unwrap().parse().unwrap();
        crops.push((
            column - 1,
            row - 1,
            params["x"].parse().unwrap(),
            params["y"].parse().unwrap(),
            params["w"].parse().unwrap(),
            params["h"].parse().unwrap(),
        ));
        search_start = absolute_position + marker.len();
    }
    crops
}

#[test]
fn kitty_transmit_only_once_across_frames() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame_a = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    let frame_b = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    let combined = format!("{}{}", frame_a, frame_b);
    assert_eq!(combined.matches("\u{1b}_Ga=t").count(), 1);
    assert!(
        frame_b.is_empty(),
        "an unchanged image must not emit cursor save/restore: {frame_b:?}"
    );
}

#[test]
fn kitty_transmission_preserves_payload_at_chunk_boundaries() {
    for length in [0, 4, 4092, 4096, 4100, 8188, 8192, 8196] {
        let payload = "QUJD".repeat(length / 4);
        let mut bytes = String::from("existing text 界");
        super::super::emit_kitty_transmit(&mut bytes, 42, 30, 40, &payload).unwrap();
        assert!(bytes.starts_with("existing text 界"));
        let commands = extract_kitty_commands(&bytes);
        assert_eq!(commands.len(), length.div_ceil(4096).max(1));
        let mut reassembled = vec![];
        for (index, command) in commands.iter().enumerate() {
            let separator = command.iter().position(|byte| *byte == b';').unwrap();
            let more = u8::from(index + 1 < commands.len());
            let control = if index == 0 {
                format!("a=t,q=2,f=32,o=z,t=d,i=42,s=30,v=40,m={more}")
            } else {
                format!("q=2,m={more}")
            };
            assert_eq!(&command[..separator], control.as_bytes());
            reassembled.extend_from_slice(&command[separator + 1..]);
        }
        assert_eq!(reassembled, payload.as_bytes(), "payload length {length}");
    }
}

#[test]
fn kitty_placement_bytes_with_negative_z() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let mut chunk = kitty_chunk(internal, 1, 5, 3);
    chunk.z_index = -1;
    let output = run_kitty_frame(&parts, vec![chunk], None);
    assert!(output.contains("\u{1b}_Ga=t,q=2,f=32,o=z,t=d,i=2000000000,s=30,v=40,m=1;"));
    assert!(output.contains("\u{1b}_Gq=2,m=0;"));
    let placement = "\u{1b}[4;6H\u{1b}[m\u{1b}_Ga=p,q=2,i=2000000000,p=1,x=0,y=0,w=30,h=40,X=0,Y=0,z=-1,C=1\u{1b}\\";
    assert!(output.contains(placement));
    let save_position = output.find("\u{1b}[s").unwrap();
    let transmit_position = output.find("\u{1b}_Ga=t").unwrap();
    let restore_position = output.rfind("\u{1b}[u").unwrap();
    assert!(save_position < transmit_position);
    assert!(transmit_position < restore_position);
}

#[test]
fn kitty_host_display_clear_forces_replacement_next_frame() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame_1 = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    assert_eq!(frame_1.matches("\u{1b}_Ga=p,q=2,").count(), 1);

    let frame_2 = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    assert!(!frame_2.contains("\u{1b}_Ga=p,q=2,"));

    let frame_3 = run_kitty_frame_with_host_clear(&parts, vec![kitty_chunk(internal, 1, 0, 0)]);
    assert!(
        frame_3.contains("\u{1b}[2J"),
        "frame must carry the host clear"
    );
    assert_eq!(
        frame_3.matches("\u{1b}_Ga=t").count(),
        1,
        "a host display clear must force the image to be re-transmitted (host may drop image data on 2J)"
    );
    assert_eq!(
        frame_3.matches("\u{1b}_Ga=p,q=2,").count(),
        1,
        "a host display clear must force the placement to be re-emitted"
    );
    assert!(
        frame_3.contains("\x1b_Ga=d,q=2,d=I,i=2000000000\x1b\\"),
        "full repaint must release the old host image before forgetting its ID"
    );
    let clear_pos = frame_3.find("\u{1b}[2J").unwrap();
    let transmit_pos = frame_3.find("\u{1b}_Ga=t").unwrap();
    let place_pos = frame_3.find("\u{1b}_Ga=p,q=2,").unwrap();
    assert!(
        clear_pos < transmit_pos && transmit_pos < place_pos,
        "after a host clear the order must be 2J, then re-transmit, then re-place"
    );
}

#[test]
fn kitty_clear_string_without_explicit_mark_does_not_invalidate_host_state() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame_1 = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 0, 0)], None);
    assert_eq!(frame_1.matches("\u{1b}_Ga=t").count(), 1);

    let frame_2 =
        run_kitty_frame_with_unmarked_clear_string(&parts, vec![kitty_chunk(internal, 1, 0, 0)]);
    assert!(
        frame_2.contains("\u{1b}[2J"),
        "frame must carry the clear string"
    );
    assert!(
        !frame_2.contains("\u{1b}_G"),
        "host state invalidation must depend on the explicit flag, not on byte sniffing"
    );
}

#[test]
fn kitty_diff_move_remove_free_retransmit() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame_1 = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 2, 2)], None);
    assert_eq!(frame_1.matches("\u{1b}_Ga=t").count(), 1);
    assert!(frame_1.contains("\u{1b}_Ga=p,q=2,i=2000000000,p=1,"));
    let frame_2 = run_kitty_frame(&parts, vec![kitty_chunk(internal, 1, 2, 5)], None);
    assert!(frame_2.contains("\u{1b}_Ga=p,q=2,i=2000000000,p=1,"));
    assert!(!frame_2.contains("a=d"));
    assert!(!frame_2.contains("\u{1b}_Ga=t"));
    let frame_3 = run_kitty_frame(&parts, vec![], None);
    assert!(frame_3.contains("\u{1b}_Ga=d,q=2,d=i,i=2000000000,p=1\u{1b}\\"));
    assert!(!frame_3.contains("d=I"));
    parts.0.borrow_mut().free(internal);
    let frame_4 = run_kitty_frame(&parts, vec![], None);
    assert!(frame_4.contains("\u{1b}_Ga=d,q=2,d=I,i=2000000000\u{1b}\\"));
    let new_internal = store_test_kitty_image(&parts.0, 30, 40);
    let frame_5 = run_kitty_frame(&parts, vec![kitty_chunk(new_internal, 2, 0, 0)], None);
    assert!(frame_5.contains("\u{1b}_Ga=t,q=2,f=32,o=z,t=d,i=2000000001,"));
}

#[test]
fn kitty_placements_of_pane_dropped_from_visible_set_are_deleted() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let both_panes_visible: HashSet<PaneId> = [
        PaneId::Terminal(TerminalId::new(1)),
        PaneId::Terminal(TerminalId::new(2)),
    ]
    .into_iter()
    .collect();
    let frame_1 = run_kitty_frame_for_panes(
        &parts,
        vec![
            (
                PaneId::Terminal(TerminalId::new(1)),
                vec![kitty_chunk(internal, 1, 0, 0)],
            ),
            (
                PaneId::Terminal(TerminalId::new(2)),
                vec![kitty_chunk(internal, 2, 0, 10)],
            ),
        ],
        both_panes_visible,
    );
    assert_eq!(frame_1.matches("\u{1b}_Ga=p,q=2,").count(), 2);
    assert!(!frame_1.contains("a=d"));

    // The first pane collapsed (say, into a stack), so it renders nothing and is no longer in
    // the visible set - its placement has to go even though it was never explicitly removed.
    let only_second_pane_visible: HashSet<PaneId> =
        [PaneId::Terminal(TerminalId::new(2))].into_iter().collect();
    let frame_2 = run_kitty_frame_for_panes(
        &parts,
        vec![(
            PaneId::Terminal(TerminalId::new(2)),
            vec![kitty_chunk(internal, 2, 0, 10)],
        )],
        only_second_pane_visible,
    );
    assert_eq!(
        frame_2.matches("\u{1b}_Ga=d,q=2,d=i,").count(),
        1,
        "the placement of the pane that left the visible set must be deleted"
    );
    assert!(
        frame_2.contains("\u{1b}_Ga=d,q=2,d=i,i=2000000000,p=1\u{1b}\\"),
        "the deleted placement must be the one belonging to the invisible pane"
    );
}

#[test]
fn kitty_occlusion_crops_exclude_covered_quarter() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 40, 80);
    let pane_geom = create_pane_geom(2, 0, 2, 2);
    let floating_panes_stack = FloatingPanesStack {
        layers: vec![pane_geom],
    };
    let mut chunk = kitty_chunk(internal, 1, 0, 0);
    chunk.source_px_width = 40;
    chunk.source_px_height = 80;
    chunk.dest_cells = (4, 4);
    let output = run_kitty_frame(&parts, vec![chunk], Some(floating_panes_stack));
    let crops = parse_kitty_placement_crops(&output);
    let crop_set: HashSet<(usize, usize, usize, usize, usize, usize)> =
        crops.iter().copied().collect();
    let expected_crop_set: HashSet<(usize, usize, usize, usize, usize, usize)> =
        [(0, 0, 0, 0, 20, 40), (0, 2, 0, 40, 40, 40)]
            .into_iter()
            .collect();
    assert_eq!(crop_set, expected_crop_set);
    let image_area = 40 * 80;
    let covered_area = (4 - 2) * 10 * (2 * 20);
    let union_area: usize = crops.iter().map(|(_, _, _, _, w, h)| w * h).sum();
    assert_eq!(union_area, image_area - covered_area);
    let covered_x_range = 20..40;
    let covered_y_range = 0..40;
    for (cell_x, cell_y, _, _, w, h) in &crops {
        let absolute_x = cell_x * 10;
        let absolute_y = cell_y * 20;
        let intersects_horizontally =
            absolute_x < covered_x_range.end && absolute_x + w > covered_x_range.start;
        let intersects_vertically =
            absolute_y < covered_y_range.end && absolute_y + h > covered_y_range.start;
        assert!(
            !(intersects_horizontally && intersects_vertically),
            "crop at ({}, {}) size {}x{} intersects the covered pane rect",
            cell_x,
            cell_y,
            w,
            h
        );
    }
}

#[test]
fn kitty_capability_gating_suppresses_all_apc() {
    let parts_with_false = create_test_kitty_parts();
    parts_with_false
        .1
        .borrow_mut()
        .insert(ClientId::new(1), false);
    let internal = store_test_kitty_image(&parts_with_false.0, 30, 40);
    let output = run_kitty_frame(
        &parts_with_false,
        vec![kitty_chunk(internal, 1, 0, 0)],
        None,
    );
    assert!(!output.contains("\u{1b}_G"));
    assert!(!parts_with_false.2.borrow().contains_key(&ClientId::new(1)));

    let parts_with_absent = create_test_kitty_parts();
    parts_with_absent.1.borrow_mut().clear();
    let internal = store_test_kitty_image(&parts_with_absent.0, 30, 40);
    let output = run_kitty_frame(
        &parts_with_absent,
        vec![kitty_chunk(internal, 1, 0, 0)],
        None,
    );
    assert!(!output.contains("\u{1b}_G"));
    assert!(!parts_with_absent.2.borrow().contains_key(&ClientId::new(1)));
}

#[test]
fn is_dirty_with_kitty_chunks_and_pending_deletes() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 30, 40);
    let mut output = create_test_kitty_output(&parts);
    let client_ids = create_test_clients(1);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&client_ids, link_handler, None);
    output.add_kitty_image_chunks_to_client(
        ClientId::new(1),
        PaneId::Terminal(TerminalId::new(1)),
        vec![kitty_chunk(internal, 1, 0, 0)],
        None,
    );
    assert!(output.is_dirty());
    assert!(output.has_rendered_assets());

    let pending_parts = create_test_kitty_parts();
    let mut transmitted = HashMap::new();
    transmitted.insert(
        (1 as InternalImageId, None),
        HostImageRecord {
            host_image_id: 2_000_000_000,
            raster_revision: 0,
        },
    );
    pending_parts.2.borrow_mut().insert(
        ClientId::new(1),
        HostKittyState {
            transmitted,
            ..Default::default()
        },
    );
    // A client held back from this frame keeps its deletes for its next one.
    let mut pending_output = create_test_kitty_output(&pending_parts);
    pending_output.add_clients(
        &create_test_clients(2)
            .into_iter()
            .filter(|client_id| *client_id != ClientId::new(1))
            .collect(),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    assert!(!pending_output.is_dirty());
    pending_output.add_clients(
        &create_test_clients(1),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    assert!(pending_output.is_dirty());
}

pub(super) fn extract_kitty_commands(output: &str) -> Vec<Vec<u8>> {
    let bytes = output.as_bytes();
    let mut commands = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b
            && index + 2 < bytes.len()
            && bytes[index + 1] == b'_'
            && bytes[index + 2] == b'G'
        {
            let body_start = index + 3;
            let mut end = body_start;
            while end + 1 < bytes.len() && !(bytes[end] == 0x1b && bytes[end + 1] == b'\\') {
                end += 1;
            }
            commands.push(bytes[body_start..end].to_vec());
            index = end + 2;
        } else {
            index += 1;
        }
    }
    commands
}

#[test]
fn kitty_emitted_bytes_roundtrip_through_our_parser() {
    let parts = create_test_kitty_parts();
    let internal = store_test_kitty_image(&parts.0, 40, 80);
    let mut chunk = kitty_chunk(internal, 1, 2, 3);
    chunk.source_px_x = 5;
    chunk.source_px_y = 7;
    chunk.source_px_width = 20;
    chunk.source_px_height = 40;
    chunk.cell_offset_x = 3;
    chunk.cell_offset_y = 4;
    chunk.z_index = -1;
    chunk.dest_cells = (2, 2);
    let output = run_kitty_frame(&parts, vec![chunk], None);
    let commands = extract_kitty_commands(&output);
    assert!(
        !commands.is_empty(),
        "expected at least one emitted kitty command"
    );

    let mut parser = KittyCommandParser::new();
    let mut decoded_image: Option<(u32, u32, KittyFormat, usize)> = None;
    let mut placement: Option<crate::panes::kitty_graphics::parser::KittyCommand> = None;
    for raw in &commands {
        match parser.parse(raw) {
            Some(Ok(command)) => match command.action {
                KittyAction::Transmit => {
                    let image = command
                        .image
                        .as_ref()
                        .expect("transmit command must carry decoded image data");
                    decoded_image =
                        Some((image.width, image.height, image.format, image.bytes.len()));
                },
                KittyAction::Display => placement = Some(command),
                _ => {},
            },
            Some(Err(err)) => panic!("emitted command failed to parse: {:?}", err.code),
            None => {},
        }
    }

    let (width, height, format, byte_len) =
        decoded_image.expect("transmit round-trip produced no decoded image");
    assert_eq!((width, height), (40, 80));
    assert_eq!(format, KittyFormat::Rgba32);
    assert_eq!(byte_len, (40 * 80 * 4) as usize);

    let placement = placement.expect("no placement command round-tripped");
    assert_eq!(placement.source_x, chunk.source_px_x as u32);
    assert_eq!(placement.source_y, chunk.source_px_y as u32);
    assert_eq!(placement.source_w, chunk.source_px_width as u32);
    assert_eq!(placement.source_h, chunk.source_px_height as u32);
    assert_eq!(placement.cell_offset_x, chunk.cell_offset_x);
    assert_eq!(placement.cell_offset_y, chunk.cell_offset_y);
    assert_eq!(placement.z_index, chunk.z_index);
    assert!(placement.suppress_cursor_movement);
}

#[test]
fn kitty_host_ids_stay_within_signed_32_bit_range() {
    let parts = create_test_kitty_parts();
    for id in 0..8u64 {
        let internal = store_test_kitty_image(&parts.0, 4, 4);
        let output = run_kitty_frame(&parts, vec![kitty_chunk(internal, id, 0, 0)], None);
        for command in extract_kitty_commands(&output) {
            let text = String::from_utf8(command).unwrap();
            for segment in text.split([',', ';']) {
                if let Some(value) = segment.strip_prefix("i=") {
                    let numeric: i64 = value.parse().unwrap();
                    assert!(
                        numeric <= i32::MAX as i64,
                        "host image id {} exceeds signed 32-bit range (Konsole parses with toInt)",
                        numeric
                    );
                }
                if let Some(value) = segment.strip_prefix("p=") {
                    let numeric: i64 = value.parse().unwrap();
                    assert!(
                        numeric <= i32::MAX as i64,
                        "host placement id {} exceeds signed 32-bit range",
                        numeric
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Unicode placeholders never reach a client that gets no image
// ---------------------------------------------------------------------------

/// A chunk of kitty unicode placeholders, the cells a virtual placement is drawn through.
fn create_placeholder_chunk(cells: usize, x: usize, y: usize) -> CharacterChunk {
    let terminal_chars: Vec<TerminalCharacter> = std::iter::repeat_n(PLACEHOLDER, cells)
        .map(TerminalCharacter::new)
        .collect();
    CharacterChunk::new(terminal_chars, x, y)
}

/// The property that actually protects a mosh client: it is told it cannot show images, so no
/// image bytes are serialized for it, and the placeholder cells that would refer to them must not
/// be serialized either - `U+10EEEE` is unassigned, and a terminal that renders it as a glyph of
/// its own choosing ends up with a screen that no longer matches its own model of it.
#[test]
fn a_client_without_kitty_support_never_receives_placeholder_cells() {
    let mut output = create_test_output_with_kitty_support(ClientId::new(1), false);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&create_test_clients(1), link_handler, None);
    output
        .add_character_chunks_to_client(
            ClientId::new(1),
            vec![create_placeholder_chunk(4, 0, 0)],
            None,
        )
        .unwrap();

    let rendered = output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap();
    assert!(
        !rendered.contains(PLACEHOLDER),
        "placeholder reached a client with no image: {rendered:?}"
    );
    // Blanked rather than dropped: the placeholder is one cell wide, so anything drawn to its
    // right on the same row would shift left if the cell simply vanished.
    assert!(rendered.ends_with("    "), "{rendered:?}");
}

#[test]
fn a_kitty_capable_client_still_receives_placeholder_cells() {
    let mut output = create_test_output_with_kitty_support(ClientId::new(1), true);
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&create_test_clients(1), link_handler, None);
    output
        .add_character_chunks_to_client(
            ClientId::new(1),
            vec![create_placeholder_chunk(4, 0, 0)],
            None,
        )
        .unwrap();

    let rendered = output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap();
    assert_eq!(rendered.matches(PLACEHOLDER).count(), 4, "{rendered:?}");
}

/// An unknown capability is not a capable one - a client whose probe has not answered yet, which
/// is every client for the first moments after it attaches.
#[test]
fn a_client_whose_capability_is_unknown_never_receives_placeholder_cells() {
    let mut output = create_test_output();
    let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
    output.add_clients(&create_test_clients(1), link_handler, None);
    output
        .add_character_chunks_to_client(
            ClientId::new(1),
            vec![create_placeholder_chunk(4, 0, 0)],
            None,
        )
        .unwrap();

    let rendered = output
        .serialize()
        .unwrap()
        .remove(&ClientId::new(1))
        .unwrap();
    assert!(!rendered.contains(PLACEHOLDER), "{rendered:?}");
}

/// The placeholder must stay one cell wide for the blanking above to be width-preserving.
#[test]
fn the_placeholder_is_one_cell_wide() {
    assert_eq!(TerminalCharacter::new(PLACEHOLDER).width(), 1);
}

/// The same `Output`, but told that `client_id`'s host presents each frame atomically.
fn create_test_output_with_synchronized_output(client_id: ClientId, supported: bool) -> Output {
    let synchronized_output_capabilities = Rc::new(RefCell::new(HashMap::new()));
    synchronized_output_capabilities
        .borrow_mut()
        .insert(client_id, supported);
    Output::new(
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(Some(SizeInPixels {
            height: 20,
            width: 10,
        }))),
        true,
        true,
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        Rc::new(RefCell::new(HashMap::new())),
        synchronized_output_capabilities,
    )
}

/// A host that shows the whole frame at once never displays the cursor trailing the chunks as
/// they paint, so it does not have to be hidden while they do - and does not then owe a `?25h`
/// and a cursor-shape CSI to undo it on every frame.
#[test]
fn a_host_that_presents_frames_atomically_is_not_sent_a_cursor_hide() {
    let client_id = ClientId::new(1);
    let mut output = create_test_output_with_synchronized_output(client_id, true);
    output.add_clients(
        &create_test_clients(1),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    output
        .add_character_chunks_to_client(
            client_id,
            vec![create_character_chunk_from_str("Test", 0, 0)],
            None,
        )
        .unwrap();
    output.hide_cursor_for_clients([client_id].into_iter());

    assert!(
        !output.cursor_was_hidden_for(client_id),
        "an atomic host owes no hide, so the cursor pass has none to undo"
    );
    let serialized = output.serialize().unwrap();
    assert!(
        !serialized.get(&client_id).unwrap().contains("\u{1b}[?25l"),
        "the frame must carry no cursor hide"
    );
}

/// The other half: a host that has not reported synchronised output keeps the hide it always had.
#[test]
fn a_host_without_synchronized_output_is_still_sent_a_cursor_hide() {
    for (label, output_under_test) in [
        (
            "explicitly unsupported",
            create_test_output_with_synchronized_output(ClientId::new(1), false),
        ),
        ("never reported", create_test_output()),
    ] {
        let client_id = ClientId::new(1);
        let mut output = output_under_test;
        output.add_clients(
            &create_test_clients(1),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output
            .add_character_chunks_to_client(
                client_id,
                vec![create_character_chunk_from_str("Test", 0, 0)],
                None,
            )
            .unwrap();
        output.hide_cursor_for_clients([client_id].into_iter());

        assert!(
            output.cursor_was_hidden_for(client_id),
            "{label}: the cursor pass has a hide to undo"
        );
        let serialized = output.serialize().unwrap();
        assert!(
            serialized.get(&client_id).unwrap().contains("\u{1b}[?25l"),
            "{label}: the frame must carry the cursor hide"
        );
    }
}

/// The watchers' frame carries the followed client's id but is delivered to the watchers'
/// terminals, so what that client's host can do says nothing about it. The override is how the
/// caller says so, in both directions.
#[test]
fn an_output_wide_override_decides_the_hide_instead_of_the_clients_own_capability() {
    for (label, capability, override_value, expect_a_hide) in [
        (
            "an atomic client, watched from a terminal that is not",
            true,
            false,
            true,
        ),
        (
            "a non-atomic client, watched from terminals that are",
            false,
            true,
            false,
        ),
    ] {
        let client_id = ClientId::new(1);
        let mut output = create_test_output_with_synchronized_output(client_id, capability);
        output.set_host_presents_frames_atomically(override_value);
        output.add_clients(
            &create_test_clients(1),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output
            .add_character_chunks_to_client(
                client_id,
                vec![create_character_chunk_from_str("Test", 0, 0)],
                None,
            )
            .unwrap();
        output.hide_cursor_for_clients([client_id].into_iter());

        assert_eq!(
            output.cursor_was_hidden_for(client_id),
            expect_a_hide,
            "{label}: the hide is the watchers' business, not the followed client's"
        );
        let serialized = output.serialize().unwrap();
        assert_eq!(
            serialized.get(&client_id).unwrap().contains("\u{1b}[?25l"),
            expect_a_hide,
            "{label}: and the frame has to match"
        );
    }
}

/// The predicate the cursor pass asks before deciding whether it owes a goto. It must ignore the
/// pass's own pre-frame hide - counting that is what made `is_dirty()` unconditionally true here
/// and left the cursor memo unable to suppress anything.
#[test]
fn only_the_frames_own_content_counts_as_having_moved_the_cursor() {
    let client_id = ClientId::new(1);
    let mut output = create_test_output();
    output.add_clients(
        &create_test_clients(1),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    assert!(
        !output.frame_moved_cursor_for(client_id),
        "an empty frame moves nothing"
    );

    output.hide_cursor_for_clients([client_id].into_iter());
    assert!(
        !output.frame_moved_cursor_for(client_id),
        "the cursor pass's own hide is not content and moves nothing"
    );

    output
        .add_character_chunks_to_client(
            client_id,
            vec![create_character_chunk_from_str("Test", 0, 0)],
            None,
        )
        .unwrap();
    assert!(
        output.frame_moved_cursor_for(client_id),
        "painted cells leave the hardware cursor after them"
    );
}

/// A pane's raw VTE passthrough, another client's fake cursor and a repainted frame all go in as
/// post-VTE instructions carrying absolute gotos, without adding a single character chunk. They
/// move the hardware cursor just as surely as chunks do, which is why the predicate cannot simply
/// be `has_rendered_assets`.
#[test]
fn a_post_vte_instruction_alone_counts_as_having_moved_the_cursor() {
    let client_id = ClientId::new(1);
    let mut output = create_test_output();
    output.add_clients(
        &create_test_clients(1),
        Rc::new(RefCell::new(LinkHandler::new())),
        None,
    );
    output.add_post_vte_instruction_to_client(client_id, "\u{1b}[5;5H\u{1b}[mx");

    assert!(
        !output.has_rendered_assets(),
        "nothing was painted, so the naive predicate would say the cursor stayed put"
    );
    assert!(
        output.frame_moved_cursor_for(client_id),
        "the goto inside the instruction moved it anyway"
    );
}

#[test]
fn cursor_is_cropped_for_agrees_with_what_serialization_emits() {
    // The caller asks with this, `serialize_with_size` acts on it, and the two only stay honest
    // if they answer the same question - so ask both about the same cursors.
    for (cursor_x, cursor_y, expected_to_be_cropped) in
        [(25, 5, true), (5, 25, true), (5, 5, false), (19, 9, false)]
    {
        let mut output = create_test_output();
        let client_ids = create_test_clients(1);
        let link_handler = Rc::new(RefCell::new(LinkHandler::new()));
        output.add_clients(&client_ids, link_handler, None);

        let max_size = Size { rows: 10, cols: 20 };
        output.set_cursor_coordinates(ClientId::new(1), cursor_x, cursor_y);
        assert_eq!(
            output.cursor_is_cropped_for(ClientId::new(1), max_size),
            expected_to_be_cropped,
            "cursor at ({cursor_x}, {cursor_y}) against {max_size:?}"
        );

        let chunk = create_character_chunk_from_str("Test", 0, 0);
        output
            .add_character_chunks_to_client(ClientId::new(1), vec![chunk], None)
            .unwrap();
        let result = output.serialize_with_size(Some(max_size)).unwrap();
        let client_output = result.get(&ClientId::new(1)).unwrap();
        assert_eq!(
            client_output.ends_with("\u{1b}[?25l"),
            expected_to_be_cropped,
            "serialization disagreed for a cursor at ({cursor_x}, {cursor_y}), got: {client_output:?}"
        );
    }
}

#[test]
fn a_client_that_has_not_placed_a_cursor_is_never_cropped() {
    let output = create_test_output();
    assert!(!output.cursor_is_cropped_for(ClientId::new(1), Size { rows: 10, cols: 20 }));
}

mod dirty_line_properties {
    use super::*;
    use crate::panes::EMPTY_TERMINAL_CHARACTER;
    use hegel::generators;
    use std::collections::BTreeSet;

    fn check(
        buffer: &OutputBuffer,
        reference: &BTreeSet<usize>,
        all: bool,
        width: usize,
        height: usize,
        x: usize,
        y: usize,
    ) {
        assert_eq!(buffer.should_update_all_lines, all);
        assert_eq!(
            buffer.changed_lines.iter().collect::<Vec<_>>(),
            reference.iter().copied().collect::<Vec<_>>()
        );
        assert_eq!(buffer.changed_lines.is_empty(), reference.is_empty());
        for line in 0..=140 {
            assert_eq!(
                buffer.changed_lines.contains(line),
                reference.contains(&line)
            );
        }
        // Short rows and a short viewport independently exercise both kinds of padding.
        let viewport: Vec<_> = (0..height / 2)
            .map(|line| {
                let mut row = Row::from_columns(VecDeque::from([TerminalCharacter::new(
                    (b'A' + (line % 26) as u8) as char,
                )]));
                row.bg_color = Some(AnsiCode::ColorIndex(17));
                row
            })
            .collect();
        let expected_lines: Vec<_> = if all {
            (0..height).collect()
        } else {
            reference
                .iter()
                .copied()
                .filter(|line| *line < height)
                .collect()
        };
        let chunks = buffer.changed_chunks_in_viewport(&viewport, width, height, x, y);
        assert_eq!(chunks.len(), expected_lines.len());
        for (chunk, line) in chunks.iter().zip(expected_lines) {
            assert_eq!((chunk.x, chunk.y), (x, line + y));
            let mut expected = vec![EMPTY_TERMINAL_CHARACTER; width];
            if line < viewport.len() {
                expected[0] = TerminalCharacter::new((b'A' + (line % 26) as u8) as char);
                for cell in &mut expected[1..] {
                    cell.styles
                        .update(|style| style.background = Some(AnsiCode::ColorIndex(17)));
                }
            }
            assert_eq!(chunk.terminal_characters, expected);
        }
        // The rectangle API deliberately includes off-viewport dirty lines, unlike chunks.
        let lines = if all {
            (0..height).collect()
        } else {
            reference.clone()
        };
        let mut rectangles = HashMap::new();
        for &start in &lines {
            if start > 0 && lines.contains(&(start - 1)) {
                continue;
            }
            let count = (start..).take_while(|line| lines.contains(line)).count();
            rectangles.insert(start, count);
        }
        assert_eq!(buffer.changed_rects_in_viewport(height), rectangles);
    }

    #[hegel::test]
    fn dirty_sequences_match_a_set(tc: hegel::TestCase) {
        let draw = |max| tc.draw(generators::integers::<usize>().max_value(max));
        let (width, height, x, y) = (1 + draw(7), draw(135), draw(10), draw(10));
        let mut buffer = OutputBuffer::default();
        let mut reference = BTreeSet::new();
        let mut all = true;
        check(&buffer, &reference, all, width, height, x, y);
        for _ in 0..40 {
            let (start, end) = (draw(140), draw(140));
            match draw(3) {
                0 => {
                    buffer.update_line(start);
                    if !all {
                        reference.insert(start);
                    }
                },
                1 => {
                    buffer.update_lines(start, end);
                    if !all {
                        reference.extend(start..=end);
                    }
                },
                2 => {
                    buffer.clear();
                    reference.clear();
                    all = false;
                },
                _ => {
                    buffer.update_all_lines();
                    reference.clear();
                    all = true;
                },
            }
            check(&buffer, &reference, all, width, height, x, y);
        }
    }

    #[test]
    fn dirty_boundaries_and_viewport_clipping() {
        let mut buffer = OutputBuffer::default();
        buffer.clear();
        let reference = BTreeSet::from([0, 63, 64, 65, 127, 128, 129]);
        for &line in &reference {
            buffer.update_line(line);
            buffer.update_line(line);
        }
        for height in [0, 1, 63, 64, 65, 127, 128, 129, 130] {
            check(&buffer, &reference, false, 4, height, 3, 5);
        }
        buffer.update_all_lines();
        check(&buffer, &BTreeSet::new(), true, 4, 65, 3, 5);
        buffer.clear();
        check(&buffer, &BTreeSet::new(), false, 4, 65, 3, 5);
    }
}

#[test]
fn viewport_clipping_counts_cells_and_blanks_partial_wide_characters() {
    let viewport = vec![Row::from_columns(
        "界語".chars().map(TerminalCharacter::new).collect(),
    )];
    let buffer = OutputBuffer::default();
    for (width, expected) in ["", " ", "界", "界 ", "界語", "界語 "]
        .into_iter()
        .enumerate()
    {
        let chunks = buffer.changed_chunks_in_viewport(&viewport, width, 1, 0, 0);
        assert_eq!(
            chunks[0]
                .terminal_characters
                .iter()
                .map(|cell| cell.character)
                .collect::<String>(),
            expected,
            "width {width}"
        );
    }
}

#[test]
fn translucency_preserves_rectangular_kitty_and_sixel_clipping() {
    use crate::ui::damage::CellRect;
    use yinz_types::colors::PaletteColor;
    let client = ClientId::new(1);
    let mut opaque = create_test_output();
    opaque.add_clients(
        &HashSet::from([client]),
        Rc::new(RefCell::new(LinkHandler::new())),
        Some(FloatingPanesStack {
            layers: vec![create_pane_geom(1, 0, 2, 1)],
        }),
    );
    let internal = store_test_kitty_image(&opaque.kitty_image_store, 30, 40);
    let mut translucent = opaque.clone();
    translucent.begin_translucent_composition(
        &HashSet::from([client]),
        vec![(
            1,
            CellRect {
                x: 1,
                y: 0,
                cols: 2,
                rows: 1,
            },
            0,
        )],
        PaletteColor::EightBit(7),
        PaletteColor::EightBit(0),
        PaletteColor::EightBit(0),
        &HashMap::new(),
    );
    for output in [&mut opaque, &mut translucent] {
        output.add_kitty_image_chunks_to_client(
            client,
            PaneId::Terminal(TerminalId::new(1)),
            vec![kitty_chunk(internal, 1, 0, 0)],
            None,
        );
        output.add_sixel_image_chunks_to_client(
            client,
            vec![SixelImageChunk {
                cell_x: 0,
                cell_y: 0,
                sixel_image_pixel_x: 0,
                sixel_image_pixel_y: 0,
                sixel_image_pixel_width: 30,
                sixel_image_pixel_height: 40,
                sixel_image_id: 1,
            }],
            None,
        );
    }
    assert!(!translucent.cursor_is_visible(1, 0, None, client));
    assert!(!opaque.sixel_chunks[&client].is_empty());
    assert_eq!(
        format!("{:?}", opaque.sixel_chunks),
        format!("{:?}", translucent.sixel_chunks)
    );
    assert_eq!(
        format!("{:?}", opaque.client_kitty_chunks),
        format!("{:?}", translucent.client_kitty_chunks)
    );
}

#[test]
fn each_tabs_links_resolve_against_that_tabs_link_table() {
    use crate::panes::terminal_character::DEFAULT_STYLES;

    // Two clients on two tabs. Each tab has its own link table, and a frame renders every tab
    // before it serializes once.
    let linked_chunk = |handler: &Rc<RefCell<LinkHandler>>, uri: &str| {
        let anchor = handler
            .borrow_mut()
            .dispatch_osc8(&[b"8", b"", uri.as_bytes()]);
        CharacterChunk::new(
            vec![TerminalCharacter::new_styled('x', DEFAULT_STYLES.link_anchor(anchor).into(),); 5],
            0,
            0,
        )
    };
    let first_tab = Rc::new(RefCell::new(LinkHandler::new()));
    let second_tab = Rc::new(RefCell::new(LinkHandler::new()));
    // Without this the second tab's table would be the shorter one and the first tab's anchor
    // would be missing from it, rather than pointing at the wrong link.
    second_tab
        .borrow_mut()
        .dispatch_osc8(&[b"8", b"", b"https://second.example/0"]);
    let first_chunk = linked_chunk(&first_tab, "https://first.example");
    let second_chunk = linked_chunk(&second_tab, "https://second.example/1");
    let rebuilt_first_chunk = CharacterChunk::new(first_chunk.terminal_characters.clone(), 0, 1);

    let mut output = create_test_output();
    let first_client = ClientId::new(1);
    let second_client = ClientId::new(2);
    output.add_clients(
        &HashSet::from([first_client]),
        first_tab,
        Some(FloatingPanesStack {
            layers: vec![create_pane_geom(2, 0, 1, 1)],
        }),
    );
    output
        .add_character_chunks_to_client(first_client, vec![first_chunk], None)
        .unwrap();
    output.add_clients(&HashSet::from([second_client]), second_tab, None);
    output
        .add_character_chunks_to_client(second_client, vec![second_chunk], None)
        .unwrap();
    // Later overlays can rebuild chunks after another tab has installed its table.
    output
        .add_character_chunks_to_client(first_client, vec![rebuilt_first_chunk], None)
        .unwrap();

    // Both fragments from middle clipping need their original table, including when a
    // watcher uses the size-limited serializer after another tab has rendered.
    for serialized in [
        output.clone().serialize().unwrap(),
        output
            .serialize_with_size(Some(Size { rows: 2, cols: 5 }))
            .unwrap(),
    ] {
        let first = &serialized[&first_client];
        let second = &serialized[&second_client];
        assert_eq!(
            first.matches("\u{1b}]8;;https://first.example").count(),
            3,
            "{first:?}"
        );
        assert!(!first.contains("second.example"), "{first:?}");
        assert!(
            second.contains("\u{1b}]8;;https://second.example/1"),
            "{second:?}"
        );
    }
}

#[test]
fn translucently_composed_links_resolve_against_their_tabs_link_table() {
    use crate::panes::terminal_character::DEFAULT_STYLES;
    use crate::ui::damage::CellRect;
    use yinz_types::colors::PaletteColor;

    // The first tab's link sits under a translucent floating pane, so it reaches the frame
    // through the composition rather than as a chunk of its own. The second tab renders after
    // it and leaves its own link table current when the frame serializes.
    let first_tab = Rc::new(RefCell::new(LinkHandler::new()));
    let second_tab = Rc::new(RefCell::new(LinkHandler::new()));
    second_tab
        .borrow_mut()
        .dispatch_osc8(&[b"8", b"", b"https://second.example/0"]);
    let anchor = first_tab
        .borrow_mut()
        .dispatch_osc8(&[b"8", b"", b"https://first.example"]);
    let linked_chunk = CharacterChunk::new(
        vec![TerminalCharacter::new_styled(
            'x',
            DEFAULT_STYLES.link_anchor(anchor).into(),
        )],
        0,
        0,
    );
    let overlay_chunk = CharacterChunk::new(vec![TerminalCharacter::new(' ')], 0, 0);

    let mut output = create_test_output();
    let first_client = ClientId::new(1);
    let second_client = ClientId::new(2);
    output.add_clients(&HashSet::from([first_client]), first_tab, None);
    output.begin_translucent_composition(
        &HashSet::from([first_client]),
        vec![(
            1,
            CellRect {
                x: 0,
                y: 0,
                cols: 1,
                rows: 1,
            },
            50,
        )],
        PaletteColor::EightBit(7),
        PaletteColor::EightBit(0),
        PaletteColor::EightBit(0),
        &HashMap::new(),
    );
    output
        .add_character_chunks_to_client(first_client, vec![linked_chunk], Some(0))
        .unwrap();
    output
        .add_character_chunks_to_client(first_client, vec![overlay_chunk], Some(1))
        .unwrap();
    output.finish_translucent_composition();
    output.add_clients(&HashSet::from([second_client]), second_tab, None);

    let serialized = output.serialize().unwrap();
    let first = &serialized[&first_client];
    assert!(
        first.contains("\u{1b}]8;;https://first.example"),
        "{first:?}"
    );
    assert!(!first.contains("second.example"), "{first:?}");
}

mod shared_style_serialization {
    use super::super::super::serialize_chunks;
    use super::*;
    use crate::panes::terminal_character::{
        AnsiStyledUnderline, CharacterStyles, LinkAnchor, RcCharacterStyles, DEFAULT_STYLES,
    };
    use hegel::generators;

    fn style(tc: &hegel::TestCase) -> CharacterStyles {
        let color = |tc: &hegel::TestCase| match tc.draw(generators::integers::<u8>().max_value(3))
        {
            0 => None,
            1 => Some(AnsiCode::Reset),
            2 => Some(AnsiCode::ColorIndex(
                tc.draw(generators::integers::<u8>().max_value(3)),
            )),
            _ => Some(AnsiCode::RgbCode((
                tc.draw(generators::integers::<u8>()),
                0,
                0,
            ))),
        };
        let on = |tc: &hegel::TestCase| match tc.draw(generators::integers::<u8>().max_value(2)) {
            0 => None,
            1 => Some(AnsiCode::Reset),
            _ => Some(AnsiCode::On),
        };
        let underline = match tc.draw(generators::integers::<u8>().max_value(3)) {
            0 => None,
            1 => Some(AnsiCode::Reset),
            2 => Some(AnsiCode::Underline(None)),
            _ => Some(AnsiCode::Underline(Some(AnsiStyledUnderline::Undercurl))),
        };
        let link = match tc.draw(generators::integers::<u8>().max_value(3)) {
            0 => None,
            1 => Some(LinkAnchor::End),
            index => Some(LinkAnchor::Start(u32::from(index - 2))),
        };
        DEFAULT_STYLES
            .foreground(color(tc))
            .background(color(tc))
            .underline_color(color(tc))
            .bold(on(tc))
            .dim(on(tc))
            .italic(on(tc))
            .reverse(on(tc))
            .strike(on(tc))
            .hidden(on(tc))
            .blink_slow(on(tc))
            .blink_fast(on(tc))
            .underline(underline)
            .link_anchor(link)
    }

    fn serialize(
        chunk: CharacterChunk,
        size: Option<Size>,
        underlines: bool,
        links: bool,
    ) -> String {
        let mut bytes = String::new();
        let handler = Rc::new(RefCell::new(LinkHandler::new()));
        handler
            .borrow_mut()
            .new_link_from_url("https://first.example".into());
        handler
            .borrow_mut()
            .new_link_from_url("https://second.example".into());
        serialize_chunks(
            &mut bytes,
            vec![chunk],
            None,
            Some(&handler),
            None,
            underlines,
            links,
            size,
            None,
        )
        .unwrap();
        bytes
    }

    /// Cells printed with one pen share a styles allocation, and the serializer skips
    /// their style diff. That must emit exactly what comparing each cell would.
    #[hegel::test]
    fn sharing_a_styles_allocation_does_not_change_the_bytes(tc: hegel::TestCase) {
        let runs = tc.draw(generators::integers::<usize>().min_value(1).max_value(6));
        let mut shared = vec![];
        let mut separate = vec![];
        for _ in 0..runs {
            let styles = style(&tc);
            let pen: RcCharacterStyles = styles.into();
            for index in 0..tc.draw(generators::integers::<usize>().min_value(1).max_value(4)) {
                let character = ['x', '\u{301}', '界'][index % 3];
                shared.push(TerminalCharacter::new_styled(character, pen.clone()));
                separate.push(TerminalCharacter::new_styled(
                    character,
                    RcCharacterStyles::Rc(Rc::new(styles)),
                ));
            }
        }
        let changed_colors = tc.draw(generators::booleans()).then(|| {
            let mut colors = [None; 256];
            colors[1] = Some(AnsiCode::RgbCode((1, 2, 3)));
            colors
        });
        let pane_default_fg = tc
            .draw(generators::booleans())
            .then_some(AnsiCode::ColorIndex(2));
        let pane_default_bg = tc
            .draw(generators::booleans())
            .then_some(AnsiCode::ColorIndex(3));
        let chunk = |characters| {
            let mut chunk = CharacterChunk::new(characters, 0, 0);
            chunk.changed_colors = changed_colors;
            chunk.pane_default_fg = pane_default_fg;
            chunk.pane_default_bg = pane_default_bg;
            chunk
        };
        let size = tc.draw(generators::booleans()).then(|| Size {
            rows: 1,
            cols: tc.draw(generators::integers::<usize>().max_value(12)),
        });
        let underlines = tc.draw(generators::booleans());
        let links = tc.draw(generators::booleans());
        assert_eq!(
            serialize(chunk(shared), size, underlines, links),
            serialize(chunk(separate), size, underlines, links)
        );
    }

    #[test]
    fn a_changed_palette_does_not_repeat_an_unchanged_style() {
        use crate::panes::terminal_character::RESET_STYLES;
        // Separate allocations, so only the style comparison can tell they are equal.
        let mut chunk = CharacterChunk::new(
            (0..3)
                .map(|_| {
                    TerminalCharacter::new_styled('x', RcCharacterStyles::Rc(Rc::new(RESET_STYLES)))
                })
                .collect(),
            0,
            0,
        );
        let mut colors = [None; 256];
        colors[1] = Some(AnsiCode::RgbCode((1, 2, 3)));
        chunk.changed_colors = Some(colors);
        let bytes = serialize(chunk, None, true, true);
        // One from the chunk's goto, one for the first cell; none for the cells after it.
        assert_eq!(bytes.matches("\u{1b}[m").count(), 2, "{bytes:?}");
        assert!(bytes.ends_with("xxx"), "{bytes:?}");
    }
}

#[test]
fn serializers_omit_empty_clients_but_preserve_control_only_output() {
    for sized in [false, true] {
        let mut output = create_test_output();
        let empty = ClientId::new(1);
        let notified = ClientId::new(2);
        output.add_clients(
            &[empty, notified].into_iter().collect(),
            Rc::new(RefCell::new(LinkHandler::new())),
            None,
        );
        output.add_post_vte_instruction_to_client(notified, "\x1b]9;hello\x1b\\");
        let frames = if sized {
            output.serialize_with_size(None).unwrap()
        } else {
            output.serialize().unwrap()
        };
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[&notified], "\x1b]9;hello\x1b\\");
    }
}
