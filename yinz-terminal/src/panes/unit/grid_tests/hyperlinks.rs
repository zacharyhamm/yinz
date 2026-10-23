use super::{create_grid_with_size_and_raw, feed_bytes, Grid};
use crate::output::{CharacterChunk, Output};
use crate::panes::terminal_character::LinkAnchor;
use crate::ClientId;

#[derive(Default)]
struct LinkedText {
    uri: String,
    glyphs: Vec<(char, String)>,
}

impl vte::Perform for LinkedText {
    fn print(&mut self, character: char) {
        if !self.uri.is_empty() {
            self.glyphs.push((character, self.uri.clone()));
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.first() == Some(&b"8".as_slice()) {
            self.uri = String::from_utf8(params[2].to_vec()).unwrap();
        }
    }
}

fn assert_serialized_link(grid: &Grid, chunks: Vec<CharacterChunk>, text: &str, uri: &str) {
    let mut output = Output::new(
        Default::default(),
        Default::default(),
        true,
        true,
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
        Default::default(),
    );
    let client = ClientId::new(1);
    output.add_clients(&[client].into(), grid.link_handler.clone(), None);
    output
        .add_character_chunks_to_client(client, chunks, None)
        .unwrap();
    let bytes = output.serialize().unwrap().remove(&client).unwrap();
    let mut linked = LinkedText::default();
    vte::Parser::new().advance(&mut linked, bytes.as_bytes());
    assert_eq!(
        linked.glyphs,
        text.chars()
            .map(|c| (c, uri.to_owned()))
            .collect::<Vec<_>>()
    );
}

fn assert_stored_link(grid: &Grid, text: &str, uri: &str) {
    let rows = grid
        .lines_above
        .iter()
        .chain(&grid.viewport)
        .chain(&grid.lines_below);
    let linked: Vec<_> = rows
        .clone()
        .flat_map(|row| &row.columns)
        .filter_map(|c| match c.styles.link_anchor {
            Some(LinkAnchor::Start(id)) => Some((c.character, id)),
            _ => None,
        })
        .collect();
    assert_eq!(linked, text.chars().map(|c| (c, 0)).collect::<Vec<_>>());
    let chunks = rows
        .enumerate()
        .map(|(y, row)| CharacterChunk::new(row.columns.iter().cloned().collect(), 0, y))
        .collect();
    assert_serialized_link(grid, chunks, text, uri);
}

#[test]
fn automatic_hyperlinks_keep_the_ascii_final_glyph_and_exclude_punctuation() {
    let url = "https://example.com/a";
    for suffix in [" ", ".,!?; next", "\r\nnext"] {
        let mut grid =
            create_grid_with_size_and_raw(4, 80, format!("note: {url}{suffix}").as_bytes());
        let chunks = grid.read_changes(0, 0).0;
        assert_serialized_link(&grid, chunks, url, url);
        assert_stored_link(&grid, url, url);
    }
}

#[test]
fn automatic_hyperlinks_keep_the_double_width_final_glyph() {
    let url = "https://example.com/界";
    for suffix in [" ", ".,!?; next", "\r\nnext"] {
        let mut grid =
            create_grid_with_size_and_raw(4, 80, format!("note: {url}{suffix}").as_bytes());
        let chunks = grid.read_changes(0, 0).0;
        assert_serialized_link(&grid, chunks, url, url);
        assert_stored_link(&grid, url, url);
    }
}

#[test]
fn automatic_hyperlinks_end_at_row_boundaries_and_span_viewport_rows() {
    for url in ["https://example.com/a", "https://example.com/界"] {
        for width in [11, 21] {
            for suffix in [" ", ". next", "\r\nnext"] {
                let mut grid =
                    create_grid_with_size_and_raw(5, width, format!("{url}{suffix}").as_bytes());
                let chunks = grid.read_changes(0, 0).0;
                assert_serialized_link(&grid, chunks, url, url);
                assert_stored_link(&grid, url, url);
            }
        }
    }
}

#[test]
fn automatic_hyperlinks_map_merged_scrollback_without_linking_preceding_text() {
    let url = format!("https://example.com/{}", "abcd".repeat(20));
    for suffix in ["., next".to_owned(), format!("{} next", ".,!;".repeat(12))] {
        // The wide prefix wraps with one unused column, so division by width is insufficient.
        let content = format!(
            "older\r\n{}\r\n{}\r\n12345678901界: {url}{suffix}",
            "before".repeat(8),
            "unrelated".repeat(5)
        );
        let mut grid = create_grid_with_size_and_raw(2, 12, content.as_bytes());
        assert!(grid.lines_above.back().unwrap().columns.len() > 5 * grid.width);
        assert_stored_link(&grid, &url, &url);

        grid.change_size(30, 12);
        assert!(grid.lines_above.is_empty());
        let chunks = grid.read_changes(0, 0).0;
        assert_serialized_link(&grid, chunks, &url, &url);
        for width in [7, 23, 12] {
            grid.change_size(3, width);
            grid.move_viewport_up(4);
            assert_stored_link(&grid, &url, &url);
            grid.reset_viewport();
            assert_stored_link(&grid, &url, &url);
        }
    }
}

#[test]
fn automatic_hyperlinks_skip_evicted_scrollback_positions() {
    let url = format!("https://example.com/{}", "abcd".repeat(20));
    for suffix in [String::new(), ".".repeat(60)] {
        let mut grid = create_grid_with_size_and_raw(2, 12, format!("{url}{suffix}").as_bytes());
        feed_bytes(&mut grid, b"\x1b[3J ");
        assert!(grid.lines_above.is_empty());
        let retained: String = grid
            .viewport
            .iter()
            .flat_map(|row| &row.columns)
            .map(|c| c.character)
            .collect();
        assert_stored_link(&grid, retained.trim_end().trim_end_matches('.'), &url);
    }
}

#[test]
fn automatic_hyperlinks_invalidate_previously_rendered_rows_on_finalization() {
    let url = "https://example.com/abcdefghijk";
    for terminator in [" ", "\r\n"] {
        let mut grid = create_grid_with_size_and_raw(6, 12, b"unrelated\r\n");
        feed_bytes(&mut grid, url.as_bytes());
        let chunks = grid.read_changes(0, 0).0;
        assert_serialized_link(&grid, chunks, "", url);
        assert!(grid.read_changes(0, 0).0.is_empty());

        feed_bytes(&mut grid, terminator.as_bytes());
        let chunks = grid.read_changes(0, 0).0;
        let rows: Vec<_> = chunks.iter().map(|chunk| chunk.y).collect();
        assert!(
            (1..=3).all(|row| rows.contains(&row)),
            "dirty rows: {rows:?}"
        );
        assert!(!rows.contains(&0));
        assert_serialized_link(&grid, chunks, url, url);
        assert!(grid.read_changes(0, 0).0.is_empty());
    }
}

#[test]
fn explicit_osc8_hyperlinks_preserve_their_target_and_final_glyph() {
    let url = "https://explicit.example";
    let text = "click 界";
    let mut grid = create_grid_with_size_and_raw(
        4,
        12,
        format!("before \x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\ after").as_bytes(),
    );
    let chunks = grid.read_changes(0, 0).0;
    assert_serialized_link(&grid, chunks, text, url);
    assert_stored_link(&grid, text, url);
}
