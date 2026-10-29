use super::super::TerminalPane;
use crate::pane::{Pane, TerminalPaneIo};
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::panes::LinkHandler;
use crate::ui::pane_boundaries_frame::FrameParams;
use insta::assert_snapshot;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use unicode_width::UnicodeWidthStr;
use yinz_types::internal_ids::ClientId;
use yinz_types::position::Position;
use yinz_types::style::{Palette, Style};
use yinz_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    pane_size::{Offset, PaneGeom, SizeInPixels},
};
use {
    yinz_types::data::{InputMode, PaletteColor},
    yinz_types::input::command::RunCommand,
};

use std::fmt::Write;

fn read_fixture(fixture_name: &str) -> Vec<u8> {
    let mut path_to_file = std::path::PathBuf::new();
    path_to_file.push("../src");
    path_to_file.push("tests");
    path_to_file.push("fixtures");
    path_to_file.push(fixture_name);
    std::fs::read(path_to_file)
        .unwrap_or_else(|_| panic!("could not read fixture {:?}", fixture_name))
}

#[test]
pub fn scrolling_inside_a_pane() {
    let fake_client_id = ClientId::new(1);
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let mut text_to_fill_pane = String::new();
    for i in 0..30 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    terminal_pane.handle_pty_bytes(text_to_fill_pane.into_bytes());
    terminal_pane.scroll_up(10, Some(fake_client_id));
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.scroll_down(3, Some(fake_client_id));
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.clear_scroll();
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn sixel_image_inside_terminal_pane() {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let sixel_image_bytes = "\u{1b}Pq
        #0;2;0;0;0#1;2;100;100;0#2;2;0;100;0
        #1~~@@vv@@~~@@~~$
        #2??}}GG}}??}}??-
        #1!14@
        \u{1b}\\";

    terminal_pane.handle_pty_bytes(Vec::from(sixel_image_bytes.as_bytes()));
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn partial_sixel_image_inside_terminal_pane() {
    // here we test to make sure we partially render an image that is partially hidden in the
    // scrollbuffer
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let pane_content = read_fixture("sixel-image-500px.six");
    terminal_pane.handle_pty_bytes(pane_content);
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn overflowing_sixel_image_inside_terminal_pane() {
    // here we test to make sure we properly render an image that overflows both in the width and
    // height of the pane
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(50);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let pane_content = read_fixture("sixel-image-500px.six");
    terminal_pane.handle_pty_bytes(pane_content);
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn scrolling_through_a_sixel_image() {
    let fake_client_id = ClientId::new(1);
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let mut text_to_fill_pane = String::new();
    for i in 0..30 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    writeln!(&mut text_to_fill_pane, "\r").unwrap();
    let pane_sixel_content = read_fixture("sixel-image-500px.six");
    terminal_pane.handle_pty_bytes(text_to_fill_pane.into_bytes());
    terminal_pane.handle_pty_bytes(pane_sixel_content);
    terminal_pane.scroll_up(10, Some(fake_client_id));
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.scroll_down(3, Some(fake_client_id));
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.clear_scroll();
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn multiple_sixel_images_in_pane() {
    let fake_client_id = ClientId::new(1);
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let mut text_to_fill_pane = String::new();
    for i in 0..5 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    writeln!(&mut text_to_fill_pane, "\r").unwrap();
    let pane_sixel_content = read_fixture("sixel-image-500px.six");
    terminal_pane.handle_pty_bytes(pane_sixel_content.clone()); // one image above text
    terminal_pane.handle_pty_bytes(text_to_fill_pane.into_bytes());
    terminal_pane.handle_pty_bytes(pane_sixel_content); // one image below text
    terminal_pane.scroll_up(20, Some(fake_client_id)); // scroll up to see both images
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn resizing_pane_with_sixel_images() {
    // here we test, for example, that sixel images don't wrap with other lines
    let fake_client_id = ClientId::new(1);
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let mut text_to_fill_pane = String::new();
    for i in 0..5 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    writeln!(&mut text_to_fill_pane, "\r").unwrap();
    let pane_sixel_content = read_fixture("sixel-image-500px.six");
    terminal_pane.handle_pty_bytes(pane_sixel_content.clone());
    terminal_pane.handle_pty_bytes(text_to_fill_pane.into_bytes());
    terminal_pane.handle_pty_bytes(pane_sixel_content);
    let mut new_win_size = PaneGeom::default();
    new_win_size.cols.set_inner(100);
    new_win_size.rows.set_inner(20);
    terminal_pane.set_geom(new_win_size);
    terminal_pane.scroll_up(20, Some(fake_client_id)); // scroll up to see both images
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn changing_character_cell_size_with_sixel_images() {
    let fake_client_id = ClientId::new(1);
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size.clone(),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index
    let mut text_to_fill_pane = String::new();
    for i in 0..5 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    writeln!(&mut text_to_fill_pane, "\r").unwrap();
    let pane_sixel_content = read_fixture("sixel-image-500px.six");
    terminal_pane.handle_pty_bytes(pane_sixel_content.clone());
    terminal_pane.handle_pty_bytes(text_to_fill_pane.into_bytes());
    terminal_pane.handle_pty_bytes(pane_sixel_content);
    // here the new_win_size is the same as the old one, we just update the character_cell_size
    // which will be picked up upon resize (which is why we're doing set_geom below)
    let mut new_win_size = PaneGeom::default();
    new_win_size.cols.set_inner(121);
    new_win_size.rows.set_inner(20);
    *character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 8,
        height: 18,
    });
    terminal_pane.set_geom(new_win_size);
    terminal_pane.scroll_up(10, Some(fake_client_id)); // scroll up to see both images
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn keep_working_after_corrupted_sixel_image() {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index

    let sixel_image_bytes = "\u{1b}PI AM CORRUPTED BWAHAHAq
        #0;2;0;0;0#1;2;100;100;0#2;2;0;100;0
        #1~~@@vv@@~~@@~~$
        #2??}}GG}}??}}??-
        #1!14@
        \u{1b}\\";

    terminal_pane.handle_pty_bytes(Vec::from(sixel_image_bytes.as_bytes()));
    let mut text_to_fill_pane = String::new();
    for i in 0..5 {
        writeln!(&mut text_to_fill_pane, "\rline {}", i + 1).unwrap();
    }
    terminal_pane.handle_pty_bytes(text_to_fill_pane.into_bytes());
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn pane_with_frame_position_is_on_frame() {
    let mut fake_win_size = PaneGeom {
        x: 10,
        y: 10,
        ..PaneGeom::default()
    };
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index

    terminal_pane.set_content_offset(Offset::frame(1));

    // row above pane: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 129)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 131)));

    // first row:  border for 10 <= col <= 130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 9)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(10, 10)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(10, 11)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(10, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(10, 129)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(10, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 131)));

    // second row: border only at col=10,130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 9)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(11, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(11, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 131)));

    // row in the middle: border only at col=10,130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 9)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(15, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(15, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 131)));

    // last row: border for 10 <= col <= 130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 9)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 10)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 11)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 131)));

    // row below pane: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 131)));
}

#[test]
pub fn pane_with_bottom_and_right_borders_position_is_on_frame() {
    let mut fake_win_size = PaneGeom {
        x: 10,
        y: 10,
        ..PaneGeom::default()
    };
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index

    terminal_pane.set_content_offset(Offset::shift(1, 1));

    // row above pane: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 129)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 131)));

    // first row: border only at col=130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 129)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(10, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 131)));

    // second row: border only at col=130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(11, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 131)));

    // row in the middle: border only at col=130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(15, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 131)));

    // last row: border for 10 <= col <= 130
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 9)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 10)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 11)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 70)));
    assert!(terminal_pane.position_is_on_frame(&Position::new(29, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 131)));

    // row below pane: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 131)));
}

fn make_terminal_pane_for_bell() -> TerminalPane {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    TerminalPane::new(
        crate::TerminalId::new(1),
        fake_win_size,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    )
}

#[test]
pub fn bell_notification_state_set_and_cleared() {
    let mut terminal_pane = make_terminal_pane_for_bell();

    assert!(
        !terminal_pane.get_bell_notification(),
        "Initially no bell notification"
    );

    terminal_pane.set_bell_notification(true);
    assert!(
        terminal_pane.get_bell_notification(),
        "Bell notification should be set"
    );

    terminal_pane.set_bell_notification(false);
    assert!(
        !terminal_pane.get_bell_notification(),
        "Bell notification should be cleared"
    );
}

#[test]
fn removing_a_client_only_clears_its_own_frame_override() {
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    let mut pane = make_terminal_pane_for_bell();
    for owner in [None, Some(other), Some(client)] {
        pane.add_highlight_pane_frame_color_override(Some("highlight".into()), owner);
        let previous_override = pane.pane_frame_color_override.clone();
        pane.set_should_render(false);
        pane.remove_client(client);
        assert_eq!(
            pane.pane_frame_color_override,
            if owner == Some(client) {
                None
            } else {
                previous_override
            }
        );
        assert_eq!(pane.should_render(), owner == Some(client));
    }
}

#[test]
fn automatic_program_titles_follow_manual_then_osc_precedence() {
    let mut pane = make_terminal_pane_for_bell();
    let raw_title = pane.current_title();
    pane.set_title_prefix(Some("yinz:topic".to_owned()));
    assert!(pane.can_lend_title());
    pane.set_running_program(Some("/bin/-zsh".to_owned()));
    assert_eq!(pane.display_title(), "yinz:topic: zsh");
    assert_eq!(pane.current_title(), raw_title);
    assert!(!pane.has_explicit_title());
    pane.set_should_render(false);
    pane.set_running_program(Some("/usr/bin/vim".to_owned()));
    assert!(pane.should_render());
    assert_eq!(pane.display_title(), "yinz:topic: vim");
    pane.set_should_render(false);
    pane.set_running_program(Some("/usr/bin/vim".to_owned()));
    assert!(!pane.should_render());
    pane.handle_pty_bytes(b"\x1b]2;editing main.rs\x07".to_vec());
    assert_eq!(pane.display_title(), "yinz:topic: editing main.rs");
    pane.rename(b"manual".to_vec());
    pane.set_running_program(Some("-zsh".to_owned()));
    assert_eq!(pane.display_title(), "yinz:topic: manual");
    pane.rename(vec![]);
    assert_eq!(pane.display_title(), "yinz:topic: editing main.rs");
    pane.grid.title = None;
    pane.set_remote_host(Some("build-box".to_owned()));
    assert_eq!(pane.display_title(), "build-box yinz:topic: zsh");
    for unknown in [None, Some("".to_owned()), Some("/".to_owned())] {
        pane.set_running_program(unknown);
        assert_eq!(pane.display_title(), "build-box yinz:topic");
    }
    pane.set_title_prefix(None);
    pane.set_remote_host(None);
    assert!(!pane.can_lend_title());
    pane.set_running_program(Some("/opt/my editor".to_owned()));
    assert_eq!(pane.display_title(), "my editor");
    assert!(pane.can_lend_title());
}

#[test]
pub fn display_title_is_unaffected_for_a_non_remote_pane() {
    let terminal_pane = make_terminal_pane_for_bell();
    assert_eq!(terminal_pane.display_title(), terminal_pane.current_title());
}

#[test]
pub fn display_title_is_bare_hostname_when_no_explicit_title_is_set() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    assert_eq!(terminal_pane.display_title(), "myhost");
}

#[test]
pub fn changing_the_remote_host_marks_the_pane_for_render() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_should_render(false);

    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    assert!(terminal_pane.should_render());

    terminal_pane.set_should_render(false);
    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    assert!(
        !terminal_pane.should_render(),
        "an unchanged host is not news"
    );
}

#[test]
pub fn display_title_prefixes_an_osc_title_with_the_hostname() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    terminal_pane.grid.title = Some("my-shell".to_owned());
    assert_eq!(terminal_pane.display_title(), "myhost: my-shell");
}

#[test]
pub fn display_title_prefixes_a_custom_rename_pane_name_with_the_hostname() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    terminal_pane.rename(b"custom-name".to_vec());
    assert_eq!(terminal_pane.display_title(), "myhost: custom-name");
}

/// The whole point of the prefix: it is not the pane's name, so it outlives every
/// writer of the name — the user's rename, the pane's own OSC title, and the idle
/// summary agent-service writes there.
#[test]
pub fn a_title_prefix_survives_every_kind_of_title() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_title_prefix(Some("yinz:topic".to_owned()));
    assert_eq!(terminal_pane.display_title(), "yinz:topic");
    terminal_pane.grid.title = Some("running the tests".to_owned());
    assert_eq!(
        terminal_pane.display_title(),
        "yinz:topic: running the tests"
    );
    terminal_pane.rename(b"custom-name".to_vec());
    assert_eq!(terminal_pane.display_title(), "yinz:topic: custom-name");
}

/// And the prefix does not become a name either: `current_title` is what the pane
/// manifest carries, and agent detection reads that title. A prefix that leaked
/// into it would let the daemon re-detect its own decoration.
#[test]
pub fn a_title_prefix_stays_out_of_the_raw_title() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    let before = terminal_pane.current_title();
    terminal_pane.set_title_prefix(Some("yinz:topic".to_owned()));
    assert_eq!(terminal_pane.current_title(), before);
    assert_eq!(terminal_pane.custom_title(), None);
    assert!(!terminal_pane.has_explicit_title());
}

#[test]
pub fn a_title_prefix_can_be_dropped() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.grid.title = Some("running the tests".to_owned());
    terminal_pane.set_title_prefix(Some("yinz:topic".to_owned()));
    terminal_pane.set_title_prefix(None);
    assert_eq!(terminal_pane.display_title(), "running the tests");
}

/// Both decorations at once, in the order they answer "where is this pane": the
/// machine, then the checkout on it.
#[test]
pub fn a_title_prefix_follows_the_hostname() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    terminal_pane.set_title_prefix(Some("yinz:topic".to_owned()));
    assert_eq!(terminal_pane.display_title(), "myhost yinz:topic");
    terminal_pane.grid.title = Some("running the tests".to_owned());
    assert_eq!(
        terminal_pane.display_title(),
        "myhost yinz:topic: running the tests"
    );
}

/// The frame is the one place the pane's id is written, so it leads whatever the
/// title turns out to be — placeholder, OSC title, or an address and a title.
#[test]
pub fn a_framed_title_leads_with_the_pane_id() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    assert_eq!(terminal_pane.framed_title(), "[1] Pane #0");
    terminal_pane.set_title_prefix(Some("yinz:topic".to_owned()));
    assert_eq!(terminal_pane.framed_title(), "[1] yinz:topic");
    terminal_pane.grid.title = Some("my-shell".to_owned());
    assert_eq!(terminal_pane.framed_title(), "[1] yinz:topic: my-shell");
}

/// `display_title` is also a tab's name when the tab holds one pane, so the id
/// stays out of it.
#[test]
pub fn a_pane_id_stays_out_of_the_display_title() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_title_prefix(Some("yinz:topic".to_owned()));
    assert_eq!(terminal_pane.display_title(), "yinz:topic");
}

#[test]
pub fn has_bell_reflects_grid_ring_bell() {
    let mut terminal_pane = make_terminal_pane_for_bell();

    assert!(
        !terminal_pane.has_bell(),
        "Initially has_bell should be false"
    );

    terminal_pane.handle_pty_bytes(vec![7u8]);
    assert!(
        terminal_pane.has_bell(),
        "has_bell should be true after pty bell byte"
    );

    terminal_pane.consume_bell();
    assert!(
        !terminal_pane.has_bell(),
        "has_bell should be false after consume_bell"
    );
}

#[test]
pub fn osc7_payload_survives_the_decoded_cwd_update() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    assert_eq!(terminal_pane.osc7_payload(), None);

    terminal_pane.handle_pty_bytes(b"\x1b]7;file://localhost/path%20with%20spaces\x1b\\".to_vec());

    assert_eq!(
        terminal_pane.drain_osc7_cwd(),
        Some(PathBuf::from("/path with spaces")),
    );
    assert_eq!(terminal_pane.drain_osc7_cwd(), None);
    assert_eq!(
        terminal_pane.osc7_payload(),
        Some("file://localhost/path%20with%20spaces"),
    );
}

#[test]
pub fn osc7_payload_updates_and_preserves_semicolons_with_either_terminator() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.handle_pty_bytes(b"\x1b]7;file://localhost/first\x1b\\".to_vec());
    assert_eq!(terminal_pane.osc7_payload(), Some("file://localhost/first"));

    terminal_pane.handle_pty_bytes(b"\x1b]7;file://localhost/a;b;c\x07".to_vec());
    assert_eq!(terminal_pane.osc7_payload(), Some("file://localhost/a;b;c"));
}

#[test]
pub fn osc7_payload_rejects_empty_non_utf8_and_control_character_payloads() {
    use vte::Perform;

    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.handle_pty_bytes(b"\x1b]7;file://localhost/tmp\x1b\\".to_vec());

    // Dispatch directly because VTE strips some controls before OSC dispatch.
    let rejected: &[&[u8]] = &[
        b"",
        b"file://host/path\x01bad",
        b"file://host/path\x7fbad",
        "file://host/path\u{9c}bad".as_bytes(),
        b"file://host/\xff\xfe",
    ];
    for payload in rejected {
        terminal_pane.grid.osc_dispatch(&[b"7", payload], false);
        assert_eq!(
            terminal_pane.osc7_payload(),
            Some("file://localhost/tmp"),
            "rejected payload {payload:?} must retain the previous URI",
        );
    }
    terminal_pane
        .grid
        .osc_dispatch(&[b"7", b"file://host/valid", b"\xff"], false);
    assert_eq!(terminal_pane.osc7_payload(), Some("file://localhost/tmp"));
}

#[test]
pub fn osc7_payload_is_cleared_by_terminal_resets() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    let report = b"\x1b]7;file://localhost/tmp\x1b\\";

    terminal_pane.handle_pty_bytes(report.to_vec());
    terminal_pane.handle_pty_bytes(b"\x1bc".to_vec());
    assert_eq!(terminal_pane.osc7_payload(), None);

    terminal_pane.handle_pty_bytes(report.to_vec());
    terminal_pane
        .grid
        .reset_terminal_state_preserving_scrollback();
    assert_eq!(terminal_pane.osc7_payload(), None);
}

#[test]
pub fn frameless_pane_position_is_on_frame() {
    let mut fake_win_size = PaneGeom {
        x: 10,
        y: 10,
        ..PaneGeom::default()
    };
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);

    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    ); // 0 is the pane index

    terminal_pane.set_content_offset(Offset::default());

    // row above pane: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 129)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(9, 131)));

    // first row: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 129)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(10, 131)));

    // second row: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(11, 131)));

    // random row in the middle: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(15, 131)));

    // last row: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 9)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(29, 131)));

    // row below pane: no border
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 10)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 11)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 70)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 130)));
    assert!(!terminal_pane.position_is_on_frame(&Position::new(30, 131)));
}

fn geom_at(x: usize, y: usize, cols: usize, rows: usize) -> PaneGeom {
    let mut geom = PaneGeom {
        x,
        y,
        ..Default::default()
    };
    geom.cols.set_inner(cols);
    geom.rows.set_inner(rows);
    geom
}

/// (x, y, cols, rows) of the pane, then of its content.
fn pane_and_content_geometry(pane: &TerminalPane) -> [usize; 8] {
    [
        pane.x(),
        pane.y(),
        pane.cols(),
        pane.rows(),
        pane.get_content_x(),
        pane.get_content_y(),
        pane.get_content_columns(),
        pane.get_content_rows(),
    ]
}

#[test]
pub fn pane_geometry_follows_override_until_reset() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_geom(geom_at(2, 3, 40, 10));
    terminal_pane.set_content_offset(Offset::frame(1));
    assert_eq!(
        pane_and_content_geometry(&terminal_pane),
        [2, 3, 40, 10, 3, 4, 38, 8],
        "the frame takes one cell off each side"
    );

    terminal_pane.set_subtitle(Some("subtitle".into()));
    assert_eq!(
        pane_and_content_geometry(&terminal_pane),
        [2, 3, 40, 10, 3, 5, 38, 7],
        "the subtitle takes a row off the top of the content"
    );

    terminal_pane.set_geom_override(geom_at(5, 6, 30, 12));
    assert_eq!(terminal_pane.current_geom(), geom_at(5, 6, 30, 12));
    assert!(terminal_pane.contains(&Position::new(6, 5)));
    assert!(!terminal_pane.contains(&Position::new(3, 2)));
    assert_eq!(
        pane_and_content_geometry(&terminal_pane),
        [5, 6, 30, 12, 6, 8, 28, 9],
        "every getter follows the override"
    );
    assert_eq!(
        (terminal_pane.grid.width, terminal_pane.grid.height),
        (28, 9),
        "the grid is resized to the override's content area"
    );

    terminal_pane.reset_size_and_position_override();
    assert_eq!(terminal_pane.current_geom(), geom_at(2, 3, 40, 10));
    assert!(terminal_pane.contains(&Position::new(3, 2)));
    assert_eq!(
        pane_and_content_geometry(&terminal_pane),
        [2, 3, 40, 10, 3, 5, 38, 7],
        "every getter is back on the pane's own geometry"
    );
    assert_eq!(
        (terminal_pane.grid.width, terminal_pane.grid.height),
        (38, 7)
    );
}

#[test]
fn pane_overlap_counts_shared_layout_spans() {
    let mut first = make_terminal_pane_for_bell();
    first.set_geom(geom_at(10, 20, 10, 10));
    let mut second = make_terminal_pane_for_bell();
    for (geom, rows, columns) in [
        (geom_at(30, 40, 5, 5), 0, 0),
        (geom_at(20, 30, 5, 5), 0, 0),
        (geom_at(12, 23, 4, 3), 3, 4),
        (geom_at(5, 15, 30, 30), 10, 10),
        (geom_at(18, 29, 4, 3), 1, 2),
        (geom_at(15, 25, 0, 0), 0, 0),
        (geom_at(10, 20, 0, 0), 0, 0),
        (geom_at(20, 30, 0, 0), 0, 0),
        (geom_at(10, 20, 10, 10), 10, 10),
    ] {
        second.set_geom(geom);
        for (pane, other) in [(&first, &second), (&second, &first)] {
            assert_eq!(pane.get_horizontal_overlap_with(other), rows, "{geom:?}");
            assert_eq!(pane.get_vertical_overlap_with(other), columns, "{geom:?}");
            assert_eq!(pane.horizontally_overlaps_with(other), rows > 0, "{geom:?}");
            assert_eq!(
                pane.vertically_overlaps_with(other),
                columns > 0,
                "{geom:?}"
            );
        }
    }
}

#[test]
fn pane_width_cap_keeps_layout_but_limits_mouse_edges() {
    use crate::pane::PaneEdge;

    let mut pane = make_terminal_pane_for_bell();
    let requested = geom_at(0, 0, 121, 20);
    pane.set_geom(requested);
    pane.set_content_offset(Offset::frame(1));
    pane.set_max_content_width(80);

    assert_eq!(pane.current_geom(), requested);
    assert_eq!(
        (pane.x(), pane.cols(), pane.right_boundary_x_coords()),
        (0, 121, 121)
    );
    assert_eq!(pane.render_geom(), geom_at(19, 0, 82, 20));
    assert_eq!((pane.get_content_x(), pane.get_content_columns()), (20, 80));
    assert_eq!((pane.grid.width, pane.grid.height), (80, 18));
    for column in [0, 18, 101, 120] {
        let position = Position::new(2, column);
        assert!(!pane.contains(&position));
        assert!(!pane.position_is_on_frame(&position));
        assert_eq!(pane.get_edge_at_position(&position), None);
    }
    for (position, edge) in [
        (Position::new(0, 19), PaneEdge::TopLeft),
        (Position::new(0, 100), PaneEdge::TopRight),
        (Position::new(19, 19), PaneEdge::BottomLeft),
        (Position::new(19, 100), PaneEdge::BottomRight),
    ] {
        assert!(pane.position_is_on_frame(&position));
        assert_eq!(pane.get_edge_at_position(&position), Some(edge));
    }
    for row in [-1, isize::MIN] {
        let mut position = Position::new(0, 19);
        position.change_line(row);
        assert!(!pane.contains(&position));
        assert!(!pane.position_is_on_frame(&position));
        assert_eq!(pane.get_edge_at_position(&position), None);
    }
    pane.set_max_content_width(0);
    assert_eq!(pane.current_geom(), requested);
    assert_eq!(pane.render_geom(), requested);
    assert_eq!(pane.grid.width, 119);
}

#[test]
fn resize_pty_clamps_dimensions_without_wrapping() {
    use crate::panes::PaneId;
    use crate::plugins::PluginInstruction;
    use crate::thread_bus::ThreadSenders;
    use crate::PtyWriteInstruction;
    use yinz_runtime::channels::{unbounded, SenderWithContext};
    use yinz_runtime::errors::prelude::*;

    let mut pane = make_terminal_pane_for_bell();
    let (tx, rx) = unbounded();
    let mut senders = ThreadSenders::default();
    senders.replace_to_pty_writer(SenderWithContext::new(tx));
    let max = u16::MAX as usize;
    for (cols, rows, cell, expected_cols, expected_rows, expected_pixels) in [
        (121, 20, None, 121, 20, None),
        (121, 20, Some((8, 21)), 121, 20, Some((968, 420))),
        (3, 5, Some((21845, 13107)), 3, 5, Some((u16::MAX, u16::MAX))),
        (4, 6, Some((21845, 13107)), 4, 6, Some((u16::MAX, u16::MAX))),
        (
            121,
            20,
            Some((usize::MAX, usize::MAX)),
            121,
            20,
            Some((u16::MAX, u16::MAX)),
        ),
        (max, max, None, u16::MAX, u16::MAX, None),
        (max + 1, max + 1, None, u16::MAX, u16::MAX, None),
        (0, 0, Some((usize::MAX, usize::MAX)), 0, 0, Some((0, 0))),
    ] {
        // Exercise PTY serialization without allocating a grid at these synthetic sizes.
        pane.geom = geom_at(0, 0, cols, rows);
        let cell = RefCell::new(cell.map(|(width, height)| SizeInPixels { width, height }));
        crate::resize_pty!(pane, (), senders, cell).unwrap();
        let (instruction, _) = rx.try_recv().unwrap();
        let PtyWriteInstruction::ResizePty(id, cols, rows, width, height) = instruction else {
            panic!("unexpected instruction: {instruction:?}");
        };
        assert_eq!(id, crate::TerminalId::new(1));
        assert_eq!((cols, rows), (expected_cols, expected_rows));
        assert_eq!(width, expected_pixels.map(|(width, _)| width));
        assert_eq!(height, expected_pixels.map(|(_, height)| height));
    }
}

fn create_guest_modal_pane() -> TerminalPane {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(80);
    fake_win_size.rows.set_inner(24);
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    TerminalPane::new(
        crate::TerminalId::new(1),
        fake_win_size,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    )
}

fn press_key(
    pane: &mut TerminalPane,
    bare_key: yinz_types::keys::BareKey,
    client_id: ClientId,
) -> Option<crate::pane::AdjustedInput> {
    let key = yinz_types::keys::KeyWithModifier::new(bare_key);
    pane.adjust_input_to_terminal(&Some(key), vec![], false, Some(client_id))
}

#[test]
fn an_ordinary_held_command_still_drops_to_a_shell_on_escape() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;

    let mut pane = create_guest_modal_pane();
    pane.hold(
        Some(0),
        false,
        RunCommand {
            command: "make".into(),
            cwd: Some(PathBuf::from("/work")),
            ..Default::default()
        },
    );
    assert!(matches!(
        press_key(&mut pane, BareKey::Esc, crate::ClientId::new(1)),
        Some(AdjustedInput::DropToShellInThisPane { .. })
    ));
}

#[test]
fn dropping_a_held_agent_to_shell_clears_agent_identity_and_command() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;

    for session_end_reported in [false, true] {
        let command = RunCommand {
            command: "env".into(),
            args: vec!["YINZ_AGENT_PANE=1".into(), "claude".into()],
            cwd: Some(PathBuf::from("/work")),
            ..Default::default()
        };
        let mut pane = agent_pane_with_run(Some(super::Run::Command(command.clone())));
        let mut agent = pane.agent_info().unwrap().clone();
        agent.session_id = Some("old-session".into());
        agent.run_id = Some("old-run".into());
        pane.set_agent_info(Some(agent));
        pane.set_detected_agent(true);
        pane.record_agent_input(true);
        pane.last_hook_at_ms = Some(123);
        if session_end_reported {
            pane.set_agent_info(None);
        }
        pane.hold(Some(0), false, command);
        assert!(pane.was_an_agent());

        assert!(matches!(
            press_key(&mut pane, BareKey::Esc, ClientId::new(1)),
            Some(AdjustedInput::DropToShellInThisPane { working_dir })
                if working_dir == Some(PathBuf::from("/work"))
        ));

        let info = crate::pane::pane_info_for_pane(&pane.pid(), &pane, &HashMap::new());
        assert!(info.agent.is_none());
        assert!(info.last_agent_session_id.is_none());
        assert!(!pane.was_an_agent());
        assert!(!pane.is_detected_agent());
        assert!(pane.last_agent_kind().is_none());
        assert!(pane.last_agent_cwd().is_none());
        assert!(pane.invoked_with().is_none());
        assert!(pane.last_hook_at_ms.is_none());
        assert_eq!(pane.agent_input_revision(), 0);
        assert_eq!(pane.agent_enter_revision(), 0);
        assert!(pane.agent_input_at().is_none());
        assert!(!pane.is_held());
    }
}

#[test]
pub fn guest_modal_navigation_wraps() {
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);
    pane.set_guest_modal(&[client_id]);
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
    press_key(&mut pane, BareKey::Up, client_id);
    assert_eq!(pane.guest_modal_selection(client_id), Some(1));
    press_key(&mut pane, BareKey::Down, client_id);
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
    press_key(&mut pane, BareKey::Down, client_id);
    assert_eq!(pane.guest_modal_selection(client_id), Some(1));
    press_key(&mut pane, BareKey::Up, client_id);
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
}

#[test]
pub fn guest_modal_tab_is_ignored() {
    use {yinz_types::data::BareKey, yinz_types::data::KeyWithModifier};
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);
    pane.set_guest_modal(&[client_id]);
    press_key(&mut pane, BareKey::Tab, client_id);
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
    let key = KeyWithModifier::new(BareKey::Tab).with_shift_modifier();
    pane.adjust_input_to_terminal(&Some(key), vec![], false, Some(client_id));
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
}

#[test]
pub fn guest_modal_enter_confirms_selection() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);

    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Enter, client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalZoom)));

    pane.set_guest_modal(&[client_id]);
    press_key(&mut pane, BareKey::Down, client_id);
    let outcome = press_key(&mut pane, BareKey::Enter, client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalDescend)));
}

#[test]
pub fn guest_modal_digit_shortcuts() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);

    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Char('1'), client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalZoom)));

    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Char('2'), client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalDescend)));
}

#[test]
pub fn guest_modal_esc_descends() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);
    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Esc, client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalDescend)));
}

#[test]
pub fn guest_modal_swallows_other_input() {
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);
    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Char('x'), client_id);
    assert!(outcome.is_none());
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
}

#[test]
pub fn guest_modal_is_per_client() {
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_a = 1;
    let client_b = 2;
    pane.set_guest_modal(&[
        crate::ClientId::new(client_a),
        crate::ClientId::new(client_b),
    ]);
    press_key(&mut pane, BareKey::Down, crate::ClientId::new(client_a));
    assert_eq!(
        pane.guest_modal_selection(crate::ClientId::new(client_a)),
        Some(1)
    );
    assert_eq!(
        pane.guest_modal_selection(crate::ClientId::new(client_b)),
        Some(0)
    );
}

#[test]
pub fn guest_modal_no_modal_passes_input_through() {
    use yinz_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId::new(1);
    let outcome = press_key(&mut pane, BareKey::Char('x'), client_id);
    assert!(matches!(
        outcome,
        Some(crate::pane::AdjustedInput::WriteBytesToTerminal(_))
    ));
}

// -- pty ingest batching ----------------------------------------------------------------------
//
// `handle_pty_bytes` forwards whole runs of non-escape bytes at once while the kitty interceptor
// is between sequences, instead of driving the state machine a byte at a time. The result has to
// be identical no matter where a pty read happens to be cut, so these feed the same bytes at
// every chunk size and compare the pane that comes out.

fn make_pane_for_ingest() -> TerminalPane {
    make_pane_for_ingest_with_cell_size(Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    }))))
}

#[test]
fn terminal_io_capability_drains_replies_without_plugin_controls() {
    let mut terminal = make_pane_for_ingest();
    let pane: &mut dyn Pane = &mut terminal;
    assert!(pane.plugin_control_mut().is_none());
    pane.handle_pty_bytes(b"\x1b[5n".to_vec());
    assert_eq!(
        pane.terminal_io_mut().unwrap().drain_messages_to_pty(),
        vec![b"\x1b[0n".to_vec()]
    );
    assert!(pane
        .terminal_io_mut()
        .unwrap()
        .drain_messages_to_pty()
        .is_empty());
}

fn make_pane_for_ingest_with_cell_size(
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
) -> TerminalPane {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);
    TerminalPane::new(
        crate::TerminalId::new(1),
        fake_win_size,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        character_cell_size,
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    )
}

#[test]
fn a_character_cell_size_change_rescales_kitty_images_without_a_geometry_change() {
    let character_cell_size = Rc::new(RefCell::new(Some(SizeInPixels {
        width: 8,
        height: 21,
    })));
    let mut pane = make_pane_for_ingest_with_cell_size(character_cell_size.clone());
    pane.handle_pty_bytes(apc_b64(
        "a=T,f=32,s=1,v=1,i=1,c=2,r=2,q=2",
        &[255, 255, 255, 255],
    ));
    let before = pane
        .render(None)
        .expect("TEST")
        .expect("kitty image render")
        .3;
    assert_eq!(before[0].scaled_px, Some((16, 42)));

    *character_cell_size.borrow_mut() = Some(SizeInPixels {
        width: 10,
        height: 20,
    });
    pane.character_cell_size_possibly_changed();

    let after = pane
        .render(None)
        .expect("TEST")
        .expect("rescaled kitty image render")
        .3;
    assert_eq!(after[0].scaled_px, Some((20, 40)));
}

/// Everything that distinguishes one ingested stream from another: what landed on the grid, how
/// many kitty commands were taken, and whether anything was left queued for a later drain.
fn ingest(bytes: &[u8], chunk_size: usize) -> (String, u64, Vec<u8>) {
    let mut pane = make_pane_for_ingest();
    for chunk in bytes.chunks(chunk_size) {
        pane.handle_pty_bytes(Vec::from(chunk));
    }
    (
        pane.grid.dump_screen(false),
        pane.grid.kitty_commands_handled(),
        pane.pending_pty_input.iter().copied().collect(),
    )
}

const INGEST_CORPUS: &[u8] = b"plain text \x1b[31mred\x1b[0m more text\r\n\
        \x1b_Gq=2,a=d,d=A\x1b\\ after the apc\r\n\
        \x1b]0;a title\x07 and an osc\r\n\
        \x1b\x1b[1mbold after a doubled escape\x1b[0m\r\n\
        \x1b_Xnot a kitty sequence\x1b\\ tail";

#[test]
pub fn pty_ingest_puts_the_expected_text_on_the_grid() {
    // The absolute oracle for the tests below, which only compare chunkings against each other:
    // a mistake in the batch loop that fires at every chunk size would be invisible to them.
    let (screen, kitty_commands_handled, pending) = ingest(INGEST_CORPUS, INGEST_CORPUS.len());

    let lines: Vec<&str> = screen.lines().map(|line| line.trim_end()).collect();
    assert_eq!(
        &lines[..5],
        &[
            "plain text red more text",
            " after the apc",
            " and an osc",
            "bold after a doubled escape",
            " tail",
        ],
        "the escape sequences should have been consumed, not printed"
    );
    assert_eq!(
        kitty_commands_handled, 1,
        "the kitty sequence should have been captured rather than forwarded to vte"
    );
    assert!(pending.is_empty(), "nothing should be left queued");
}

#[test]
pub fn pty_ingest_is_identical_however_the_bytes_are_chunked() {
    let whole = ingest(INGEST_CORPUS, INGEST_CORPUS.len());
    for chunk_size in [1, 2, 3, 5, 13, 64, INGEST_CORPUS.len()] {
        assert_eq!(
            ingest(INGEST_CORPUS, chunk_size),
            whole,
            "chunk size {}",
            chunk_size
        );
    }
}

#[test]
pub fn pty_ingest_handles_escapes_at_the_edges_of_a_read() {
    // the batch loop finds the run of ordinary bytes before the next escape; these are the inputs
    // where that run is empty, is the whole buffer, or ends the buffer
    let inputs: Vec<&[u8]> = vec![
        b"\x1b",
        b"\x1bA",
        b"A\x1b",
        b"\x1b\x1b",
        b"\x1b\x1b\x1b\x1b",
        b"no escapes here at all",
        b"\x1b_Gq=2,a=d,d=A\x1b\\",
        b"A\x1b_Gq=2,a=d,d=A\x1b\\B\x1b_Gq=2,a=d,d=B\x1b\\C",
    ];
    // two captured sequences, and only the surrounding letters reach the grid
    let (screen, kitty_commands_handled, _) = ingest(inputs[7], usize::MAX);
    assert_eq!(screen.lines().next().unwrap().trim_end(), "ABC");
    assert_eq!(kitty_commands_handled, 2);

    for input in inputs {
        let whole = ingest(input, input.len().max(1));
        for chunk_size in 1..=input.len().max(1) {
            assert_eq!(
                ingest(input, chunk_size),
                whole,
                "input {:?} at chunk size {}",
                input,
                chunk_size
            );
        }
    }
}

#[test]
fn ordinary_escape_runs_preserve_every_read_split() {
    for prefix in [b"\x1b".as_slice(), b"\x1b_"] {
        let mut stream = INGEST_CORPUS.to_vec();
        // Repeated ESCs before both ordinary controls and APC candidates, including non-Kitty APCs.
        stream.extend_from_slice(b"\x1b\x1b\x1b[31ma\x1b[m\x1b\x1b_other\x1b\\b\x1b_\x1b[32mc");
        stream.extend_from_slice(prefix);
        let completion = &b"\x1b_Gq=2,a=d,d=A\x1b\\after"[prefix.len()..];
        let expected = ingest(&[stream.as_slice(), completion].concat(), 1);
        for split in 0..=stream.len() {
            let mut pane = make_pane_for_ingest();
            for read in [&stream[..split], &stream[split..], completion] {
                pane.handle_pty_bytes(read.to_vec());
            }
            assert_eq!(
                (
                    pane.grid.dump_screen(false),
                    pane.grid.kitty_commands_handled(),
                    pane.drain_pending_pty_input(),
                ),
                expected,
                "prefix {prefix:?}, split {split}"
            );
        }
    }
}

#[test]
pub fn a_kitty_sequence_split_across_two_reads_is_still_captured() {
    // the interceptor is mid-sequence when the second read arrives, so that read must not be
    // batched as a run of ordinary bytes
    let mut pane = make_pane_for_ingest();
    pane.handle_pty_bytes(Vec::from(&b"A\x1b_Gq=2,a=d,"[..]));
    assert_eq!(
        pane.grid.kitty_commands_handled(),
        0,
        "the sequence has not terminated yet"
    );

    pane.handle_pty_bytes(Vec::from(&b"d=A\x1b\\B"[..]));

    assert_eq!(pane.grid.kitty_commands_handled(), 1);
    assert_eq!(
        pane.grid
            .dump_screen(false)
            .lines()
            .next()
            .unwrap()
            .trim_end(),
        "AB",
        "only the letters around the sequence should reach the grid"
    );

    // and the split delivery is indistinguishable from a single one
    let (whole_screen, whole_commands, _) = ingest(b"A\x1b_Gq=2,a=d,d=A\x1b\\B", usize::MAX);
    assert_eq!(pane.grid.dump_screen(false), whole_screen);
    assert_eq!(pane.grid.kitty_commands_handled(), whole_commands);
}

#[test]
pub fn a_forward_interrupting_the_flush_after_a_batched_run_requeues_from_the_escape() {
    // An OSC that needs the host, ordinary bytes, then a Kitty capture that triggers the flush.
    // Pausing must retain both the unconsumed text and the entire Kitty prefix for replay.
    let mut pane = make_pane_for_ingest();
    pane.handle_pty_bytes(Vec::from(
        &b"\x1b]10;?\x07plain\x1b_Gq=2,a=d,d=A\x1b\\tail"[..],
    ));

    assert_eq!(
        pane.drain_forwarded_queries().len(),
        1,
        "the OSC 10 query should have been forwarded to the host"
    );
    assert_eq!(
        pane.drain_pending_pty_input(),
        b"plain\x1b_Gq=2,a=d,d=A\x1b\\tail".to_vec(),
        "the batched run vte had not consumed yet, followed by everything from the kitty \
         sequence's escape onwards, must be replayed after the reply"
    );
    assert_eq!(
        pane.grid.kitty_commands_handled(),
        0,
        "the kitty sequence was queued, not handled, so it is not lost"
    );
}

#[test]
fn host_query_pause_and_replay_preserve_every_input_split() {
    for terminator in ["\x07", "\x1b\\"] {
        for (tail, expected_screen) in [
            ("after\x1b[31mred\x1b[m\x1b[6n", "beforeafterred"),
            // A borrowed run, a reconstructed non-Kitty prefix, and another run share a batch.
            ("after\x1b_Xignored\x1b\\red\x1b[6n", "beforeafterred"),
            ("after\x1b_Gq=2,a=d,d=A\x1b\\red\x1b[6n", "beforeafterred"),
            // The query fills the whole parser batch before the capture starts.
            ("\x1b_Gq=2,a=d,d=A\x1b\\red\x1b[6n", "beforered"),
        ] {
            let stream = format!("before\x1b]10;?{terminator}{tail}");
            // vte dispatches an OSC on the ESC of ST, leaving its backslash to replay.
            let expected_pending =
                format!("{}{tail}", if terminator == "\x1b\\" { "\\" } else { "" });
            for split in 0..=stream.len() {
                let mut pane = make_pane_for_ingest();
                let mut forwarded = 0;
                for read in [&stream.as_bytes()[..split], &stream.as_bytes()[split..]] {
                    pane.handle_pty_bytes(read.to_vec());
                    let queries = pane.drain_forwarded_queries();
                    forwarded += queries.len();
                    if !queries.is_empty() {
                        // The same pause that Tab arms after draining a host query.
                        pane.arm_forward_pause();
                    }
                }
                assert_eq!(forwarded, 1, "split {split}: {stream:?}");
                assert_eq!(pane.grid.dump_screen(false).trim_end(), "before");
                assert!(pane.drain_messages_to_pty().is_empty());
                let pending = pane.drain_pending_pty_input();
                assert_eq!(pending, expected_pending.as_bytes(), "split {split}");
                assert!(pane.clear_forward_pause());
                pane.handle_pty_bytes(pending);
                assert_eq!(pane.grid.dump_screen(false).trim_end(), expected_screen);
                assert_eq!(
                    pane.grid.kitty_commands_handled(),
                    u64::from(tail.contains("\x1b_G"))
                );
                assert_eq!(
                    pane.drain_messages_to_pty(),
                    vec![format!("\x1b[1;{}R", expected_screen.len() + 1).into_bytes()]
                );
                assert!(pane.drain_forwarded_queries().is_empty());
                assert!(pane.drain_pending_pty_input().is_empty());
            }
        }
    }
}

#[test]
fn mixed_sgr_and_kitty_output_flushes_only_at_capture_boundaries() {
    let mut pane = make_pane_for_ingest();
    let mut stream = b"\x1b[31ma\x1b[m\x1b[2;3H".to_vec();
    stream.extend(apc_b64("a=T,f=32,s=1,v=1,i=1,C=1,q=2", &[255; 4]));
    stream.extend_from_slice(b"\x1b[32mB\x1b[m\x1b[1mC\x1b[m");
    pane.handle_pty_bytes(stream);

    assert_eq!(
        pane.vte_batches, 2,
        "one batch on each side of the Kitty APC"
    );
    let placement = &pane.grid.kitty_placements()[0];
    assert_eq!(
        (placement.display_rect.x, placement.display_rect.y),
        (16, 21)
    );
    let screen = pane.grid.dump_screen(false);
    let lines: Vec<_> = screen.lines().collect();
    assert_eq!(lines[0].trim_end(), "a");
    assert_eq!(lines[1].trim_end(), "  BC");
}

#[test]
fn host_query_pause_replays_held_prefix_before_a_later_read() {
    for prefix in ["\x1b", "\x1b_"] {
        let mut pane = make_pane_for_ingest();
        pane.handle_pty_bytes(format!("before\x1b]10;?\x07plain{prefix}").into_bytes());
        assert_eq!(pane.drain_forwarded_queries().len(), 1);
        pane.arm_forward_pause();
        let pending = pane.drain_pending_pty_input();
        assert_eq!(pending, format!("plain{prefix}").as_bytes());
        pane.clear_forward_pause();
        pane.handle_pty_bytes(pending);
        assert_eq!(pane.grid.dump_screen(false).trim_end(), "beforeplain");

        let command = b"\x1b_Gq=2,a=d,d=A\x1b\\after";
        pane.handle_pty_bytes(command[prefix.len()..].to_vec());
        assert_eq!(pane.grid.kitty_commands_handled(), 1);
        assert_eq!(pane.grid.dump_screen(false).trim_end(), "beforeplainafter");
        assert!(pane.drain_pending_pty_input().is_empty());
    }
}

fn suspended_pane() -> TerminalPane {
    let mut pane = create_guest_modal_pane();
    pane.set_suspended(true);
    pane
}

/// A suspended pane's process cannot read its pty, so forwarding keystrokes to
/// it would only fill a buffer nobody will ever drain. The two keys that mean
/// something out here are the only ones that do anything.
#[test]
fn a_suspended_pane_resumes_on_enter_and_swallows_everything_else() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;

    let mut pane = suspended_pane();
    assert!(matches!(
        press_key(&mut pane, BareKey::Enter, crate::ClientId::new(1)),
        Some(AdjustedInput::ResumeSuspendedProcessInThisPane)
    ));

    let mut pane = suspended_pane();
    assert!(matches!(
        pane.adjust_input_to_terminal(&None, vec![b' '], false, Some(crate::ClientId::new(1))),
        Some(AdjustedInput::ResumeSuspendedProcessInThisPane)
    ));

    let mut pane = suspended_pane();
    assert!(matches!(
        pane.adjust_input_to_terminal(&None, vec![3], false, Some(crate::ClientId::new(1))),
        Some(AdjustedInput::CloseThisPane)
    ));

    let mut pane = suspended_pane();
    assert!(
        press_key(&mut pane, BareKey::Char('x'), crate::ClientId::new(1)).is_none(),
        "an ordinary key must not reach a stopped process"
    );

    // Esc means "drop to shell" for a held pane; a suspended pane has a live
    // process and nothing to drop out of.
    let mut pane = suspended_pane();
    assert!(press_key(&mut pane, BareKey::Esc, crate::ClientId::new(1)).is_none());
}

/// Once the process is gone the resume hint is a lie, and the pane would keep
/// swallowing every key with no way back.
#[test]
fn holding_a_suspended_pane_clears_its_suspension() {
    let mut pane = suspended_pane();
    assert!(pane.is_suspended());
    pane.hold(Some(1), false, RunCommand::default());
    assert!(!pane.is_suspended());
}

/// A pane that suspends and then goes back to running must forward keys again -
/// `WCONTINUED` is what makes an external `kill -CONT` clear this too, not just
/// our own resume.
#[test]
fn a_resumed_pane_forwards_input_again() {
    use crate::pane::AdjustedInput;
    use yinz_types::keys::BareKey;

    let mut pane = suspended_pane();
    pane.set_suspended(false);
    assert!(matches!(
        press_key(&mut pane, BareKey::Char('x'), crate::ClientId::new(1)),
        Some(AdjustedInput::WriteBytesToTerminal(_))
    ));
}

#[test]
pub fn waiting_agent_bell_persists_until_the_reported_wait_ends() {
    let waiting = AgentInfo {
        kind: "claude".to_owned(),
        state: AgentState::Waiting,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: Some("Bash rm".to_owned()),
        wait_needs_a_person: true,
        cwd: None,
        session_id: Some("s1".to_owned()),
        run_id: None,
    };
    let mut pane = agent_pane();
    pane.update_name("my agent");
    let render = |pane: &mut TerminalPane, focused| {
        pane.render_frame(ClientId::new(1), frame_params(focused), InputMode::Normal)
            .unwrap()
            .unwrap()
            .0
            .into_iter()
            .flat_map(|chunk| chunk.terminal_characters)
            .map(|character| character.character)
            .collect::<String>()
    };
    for focused in [false, true, false, true] {
        pane.set_agent_info(Some(waiting.clone()));
        assert!(render(&mut pane, focused).contains("🔔 [1] my agent"));
        assert_eq!(pane.stack_list_entry_label(false), "🔔 [1] my agent");
        assert_eq!(
            pane.display_title(),
            "my agent",
            "lent titles stay undecorated"
        );
        assert_eq!(pane.custom_title().as_deref(), Some("my agent"));
    }
    let mut quiet = waiting.clone();
    quiet.wait_needs_a_person = false;
    for next in [
        Some(AgentInfo {
            state: AgentState::Busy,
            ..waiting.clone()
        }),
        Some(AgentInfo {
            state: AgentState::Idle,
            ..waiting.clone()
        }),
        Some(AgentInfo {
            state: AgentState::Exited,
            ..waiting.clone()
        }),
        Some(quiet),
        None,
    ] {
        pane.set_agent_info(Some(waiting.clone()));
        pane.render_frame(ClientId::new(1), frame_params(true), InputMode::Normal)
            .unwrap();
        pane.set_should_render(false);
        assert!(pane.set_agent_info(next));
        assert!(pane.should_render(), "reports repaint idle panes");
        assert!(!render(&mut pane, true).contains('🔔'));
        assert_eq!(pane.stack_list_entry_label(false), "[1] my agent");
    }
    pane.set_agent_info(Some(waiting));
    pane.set_bell_notification(true);
    assert_eq!(pane.stack_list_entry_label(false), "🔔 [1] my agent [!]");
    pane.set_bell_notification(false);
    assert_eq!(pane.stack_list_entry_label(false), "🔔 [1] my agent");
}

/// A pane the host opened for `agent_spawn` starts with the kind the planner resolved,
/// before the process has said anything. opencode is one of those kinds, so the pane it
/// opens is an agent pane in the manifest from the first frame - which is what agent and
/// agents read.
#[test]
pub fn a_spawned_opencode_pane_records_its_kind_before_it_has_said_anything() {
    yinz_types::agent::provider::register(
        0,
        yinz_types::agent::provider::AgentProvider {
            kind: "opencode".into(),
            executable: "opencode".into(),
            ..Default::default()
        },
    )
    .unwrap();
    use yinz_types::data::{OriginatingPlugin, AGENT_PANE_CONTEXT};
    let run = |marker: &str, program: &str| {
        super::Run::Command(RunCommand {
            command: PathBuf::from(program),
            args: vec!["--auto".to_owned(), "--prompt".to_owned(), "go".to_owned()],
            originating_plugin: Some(OriginatingPlugin::new(
                yinz_types::internal_ids::PluginId::new(2),
                crate::ClientId::new(1),
                std::collections::BTreeMap::from([(
                    AGENT_PANE_CONTEXT.to_owned(),
                    marker.to_owned(),
                )]),
            )),
            ..Default::default()
        })
    };
    // What the host writes: the program the planner chose, as the marker's value.
    let agent = super::agent_from_run(Some(&run("opencode", "opencode")))
        .expect("a marked command pane starts with an agent record");
    assert_eq!(agent.kind, "opencode");
    assert_eq!(agent.source, AgentSource::Spawned);
    assert_eq!(agent.state, AgentState::Busy);
    // The legacy `"true"` marker falls back to the program's name, which is the same kind.
    let legacy = super::agent_from_run(Some(&run("true", "opencode")))
        .expect("the legacy marker still names the agent");
    assert_eq!(legacy.kind, "opencode");
    // The CLI launch marker identifies an agent without inventing a plugin owner.
    let direct = super::agent_from_run(Some(&super::Run::Command(RunCommand {
        command: PathBuf::from("env"),
        args: vec![
            format!("{}=1", yinz_types::agent::AGENT_PANE_ENV),
            "opencode".into(),
        ],
        ..Default::default()
    })))
    .unwrap();
    assert_eq!(direct.kind, "opencode");
    assert_eq!(direct.source, AgentSource::Spawned);
    // Existing layouts and ordinary command panes retain their previous unmarked behavior.
    assert!(super::agent_from_run(Some(&super::Run::Command(RunCommand {
        command: PathBuf::from("opencode"),
        ..Default::default()
    })))
    .is_none());
    assert!(super::agent_from_run(Some(&super::Run::Command(RunCommand {
        command: PathBuf::from("env"),
        args: vec![
            "opencode".into(),
            format!("{}=1", yinz_types::agent::AGENT_PANE_ENV),
        ],
        ..Default::default()
    })))
    .is_none());
    let mut unmarked_plugin = run("opencode", "opencode");
    if let super::Run::Command(command) = &mut unmarked_plugin {
        command.originating_plugin.as_mut().unwrap().context.clear();
    }
    assert!(super::agent_from_run(Some(&unmarked_plugin)).is_none());
    assert!(super::agent_from_run(Some(&super::Run::Command(RunCommand {
        command: PathBuf::from("zsh"),
        ..Default::default()
    })))
    .is_none());
}

#[test]
fn bubblewrap_launches_record_agents_before_output() {
    yinz_types::agent::provider::register(
        0,
        yinz_types::agent::provider::AgentProvider {
            kind: "opencode".into(),
            executable: "opencode".into(),
            ..Default::default()
        },
    )
    .unwrap();
    for (kind, inner) in [
        ("claude", vec!["claude"]),
        ("opencode", vec!["opencode"]),
        (
            "codex",
            vec![
                "/usr/bin/yinz",
                "agent",
                "codex",
                "--command",
                "/opt/codex",
                "--",
            ],
        ),
    ] {
        let mut args = vec!["YINZ_AGENT_PANE=1"];
        args.extend(inner);
        // The sandbox is the pane's target; its command is the agent's own.
        let command = RunCommand {
            command: "/usr/bin/env".into(),
            args: args.into_iter().map(str::to_owned).collect(),
            cwd: Some("/project/.worktrees/task".into()),
            target: yinz_types::command::PaneTarget::Bubblewrap {
                root: "/project".into(),
            },
            ..Default::default()
        };
        for marker in [None, Some(kind), Some("true"), Some("")] {
            let mut command = command.clone();
            if let Some(marker) = marker {
                command.originating_plugin = Some(yinz_types::data::OriginatingPlugin::new(
                    yinz_types::internal_ids::PluginId::new(2),
                    crate::ClientId::new(1),
                    std::collections::BTreeMap::from([(
                        yinz_types::data::AGENT_PANE_CONTEXT.into(),
                        marker.into(),
                    )]),
                ));
            }
            let pane = agent_pane_with_run(Some(super::Run::Command(command)));
            let agent = pane
                .agent_info()
                .expect("agent record exists before any output or hooks");
            assert_eq!(agent.kind, kind);
            assert_eq!(agent.source, AgentSource::Spawned);
            assert_eq!(agent.state, AgentState::Busy);
            assert_eq!(agent.cwd.as_deref(), Some("/project/.worktrees/task"));
        }
    }
}

fn agent_pane() -> TerminalPane {
    agent_pane_with_run(None)
}

fn agent_pane_with_run(invoked_with: Option<super::Run>) -> TerminalPane {
    let mut geom = PaneGeom::default();
    geom.cols.set_inner(80);
    geom.rows.set_inner(20);
    TerminalPane::new(
        crate::TerminalId::new(1),
        geom,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        None,
        invoked_with,
        false, // debug
        true,  // arrow_fonts
        true,  // styled_underlines
        true,  // osc8_hyperlinks
        false, // explicitly_disable_kitty_keyboard_protocol
        None,
    )
}

#[test]
fn waiting_pulse_uses_elapsed_time_and_rearms_only_for_a_new_wait() {
    let mut pane = agent_pane();
    let mut waiting = AgentInfo {
        kind: "claude".into(),
        state: AgentState::Waiting,
        source: AgentSource::Reported,
        detail: None,
        waiting_on: Some("approval".into()),
        wait_needs_a_person: true,
        cwd: None,
        session_id: None,
        run_id: None,
    };
    pane.set_agent_info(Some(waiting.clone()));
    let start = pane.waiting_pulse_started_at().unwrap();
    for (millis, strength) in [
        (0, 0.0),
        (450, 0.35),
        (900, 0.7),
        (1350, 0.35),
        (1800, 0.0),
        (2700, 0.7),
        (3600, 0.0),
    ] {
        pane.set_should_render(false);
        pane.sample_waiting_pulse(start + std::time::Duration::from_millis(millis), true);
        assert_eq!(pane.waiting_pulse(), Some(strength));
        assert!(
            pane.should_render(),
            "an idle pane repaints when the sample changes"
        );
    }
    // Earlier, late and repeated samples never accumulate a per-tick step.
    for millis in [901, 12, 1377, 2700, 2700] {
        pane.sample_waiting_pulse(start + std::time::Duration::from_millis(millis), true);
    }
    assert_eq!(pane.waiting_pulse(), Some(0.7));
    pane.sample_waiting_pulse(start, false);
    assert_eq!(pane.waiting_pulse(), None);
    assert_eq!(
        pane.waiting_pulse_started_at(),
        Some(start),
        "disabled effects retain attention"
    );
    assert!(!pane.set_agent_info(Some(waiting.clone())));
    waiting.detail = Some("updated detail".into());
    waiting.session_id = Some("metadata".into());
    pane.set_agent_info(Some(waiting.clone()));
    assert_eq!(pane.waiting_pulse_started_at(), Some(start));

    pane.sample_waiting_pulse(start + std::time::Duration::from_millis(900), true);
    pane.set_agent_info(Some(waiting.clone()));
    waiting.cwd = Some("/work".into());
    pane.set_agent_info(Some(waiting.clone()));
    assert_eq!(pane.waiting_pulse_started_at(), Some(start));
    assert_eq!(pane.waiting_pulse(), Some(0.7));
    assert!(pane.stack_list_entry_label(false).contains('🔔'));

    for next in [
        None,
        Some(AgentInfo {
            state: AgentState::Busy,
            ..waiting.clone()
        }),
        Some(AgentInfo {
            wait_needs_a_person: false,
            ..waiting.clone()
        }),
    ] {
        pane.set_agent_info(next);
        assert_eq!(pane.waiting_pulse_started_at(), None);
        assert_eq!(pane.waiting_pulse(), None);
        assert!(!pane.stack_list_entry_label(false).contains('🔔'));
        pane.sample_waiting_pulse(start, true);
        assert_eq!(pane.waiting_pulse(), None);
        pane.set_agent_info(Some(waiting.clone()));
        assert!(
            pane.waiting_pulse_started_at().is_some(),
            "a new human wait re-arms attention"
        );
        pane.sample_waiting_pulse(
            pane.waiting_pulse_started_at().unwrap() + std::time::Duration::from_millis(900),
            true,
        );
        assert_eq!(pane.waiting_pulse(), Some(0.7));
        assert!(pane.stack_list_entry_label(false).contains('🔔'));
    }
    pane.hold(Some(0), false, RunCommand::default());
    assert_eq!(pane.waiting_pulse_started_at(), None);
    assert_eq!(pane.waiting_pulse(), None);
    assert!(!pane.stack_list_entry_label(false).contains('🔔'));
    pane.set_agent_info(Some(waiting.clone()));
    assert!(pane.waiting_pulse_started_at().is_some());
    pane.respawn(Some(RunCommand::default()), false).unwrap();
    assert_eq!(pane.waiting_pulse_started_at(), None);
    assert_eq!(pane.waiting_pulse(), None);
    assert!(!pane.stack_list_entry_label(false).contains('🔔'));
}

fn frame_params(is_main_client: bool) -> FrameParams {
    FrameParams {
        focused_client: is_main_client.then_some(crate::ClientId::new(1)),
        is_main_client,
        other_focused_clients: vec![],
        style: Style::default(),
        color: None,
        other_cursors_exist_in_session: false,
        pane_is_stacked_over: false,
        pane_is_stacked_under: false,
        pane_is_stacked: false,
        should_draw_pane_frames: true,
        pane_is_floating: false,
        show_pinned_indication: false,
        show_scroll_indication: false,
        content_offset: Offset::default(),
        mouse_is_hovering_over_pane: false,
        pane_is_selectable: true,
        show_help_text: false,
        highlight_tooltip: None,
        omit_title: false,
        frame_geom_override: None,
        stack_list_entry: None,
        blank_title: false,
        same_worktree: false,
        mouse_scroll_resize: false,
        dimmed: false,
        guest_choice_indicator: None,
        opening_ripple: None,
    }
}

#[test]
fn same_worktree_title_respects_blank_omitted_narrow_and_cached_frames() {
    let mut pane = agent_pane();
    pane.update_name("my agent");
    let params = || {
        let mut params = frame_params(false);
        params.same_worktree = true;
        params
    };
    let render = |pane: &mut TerminalPane, params| {
        pane.render_frame(ClientId::new(1), params, InputMode::Normal)
            .unwrap()
            .unwrap()
            .0
            .into_iter()
            .flat_map(|chunk| chunk.terminal_characters)
            .map(|character| character.character)
            .collect::<String>()
    };
    assert!(render(&mut pane, params()).contains("👯 [1] my agent"));
    assert!(pane
        .render_frame(ClientId::new(1), params(), InputMode::Normal)
        .unwrap()
        .is_none());
    assert_eq!(pane.stack_list_entry_label(true), "👯 [1] my agent");
    assert_eq!(pane.display_title(), "my agent");
    assert!(!render(&mut pane, frame_params(false)).contains('👯'));
    for (blank, omit) in [(true, false), (false, true)] {
        let mut hidden = params();
        hidden.blank_title = blank;
        hidden.omit_title = omit;
        assert!(!render(&mut pane, hidden).contains('👯'));
    }
    for cols in 5..30 {
        let mut geom = pane.current_geom();
        geom.cols.set_inner(cols);
        pane.set_geom(geom);
        let chunks = pane
            .render_frame(ClientId::new(1), params(), InputMode::Normal)
            .unwrap()
            .unwrap()
            .0;
        for chunk in chunks {
            let width: usize = chunk.terminal_characters.iter().map(|c| c.width()).sum();
            assert!(chunk.x + width <= cols, "title overflow at {cols} columns");
        }
    }
}

#[test]
fn frames_are_cached_per_client_and_borderless_frames_are_not_cached() {
    let client = ClientId::new(1);
    let other = ClientId::new(2);
    let mut pane = agent_pane();
    let render = |pane: &mut TerminalPane, client| {
        pane.render_frame(client, frame_params(true), InputMode::Normal)
            .unwrap()
            .is_some()
    };
    assert!(render(&mut pane, client));
    assert!(!render(&mut pane, client));
    assert!(render(&mut pane, other));
    pane.set_borderless(true);
    pane.update_name("renamed");
    assert!(!render(&mut pane, client));
    pane.set_borderless(false);
    assert!(render(&mut pane, client));
    assert!(!render(&mut pane, client));
}

#[test]
fn terminal_pane_grid_shows_ab_around_captured_apc() {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);
    let pid = 1;
    let style = Style::default();
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    let debug = false;
    let arrow_fonts = true;
    let styled_underlines = true;
    let osc8_hyperlinks = true;
    let explicitly_disable_kitty_keyboard_protocol = false;
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(pid),
        fake_win_size,
        style,
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        sixel_image_store,
        Rc::new(RefCell::new(KittyImageStore::default())),
        terminal_emulator_colors,
        terminal_emulator_color_codes,
        None,
        None,
        debug,
        arrow_fonts,
        styled_underlines,
        osc8_hyperlinks,
        explicitly_disable_kitty_keyboard_protocol,
        None,
    );
    terminal_pane.handle_pty_bytes(Vec::from(&b"A\x1b_Gq=2,a=d,d=A\x1b\\B"[..]));
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    assert_eq!(terminal_pane.grid.kitty_commands_handled(), 1);
}

fn apc_b64(control: &str, payload: &[u8]) -> Vec<u8> {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(payload);
    let separator = if payload.is_empty() { "" } else { ";" };
    format!("\x1b_G{control}{separator}{payload}\x1b\\").into_bytes()
}

#[test]
fn agent_double_escape_frame_prefix_restores_title_and_respects_geometry() {
    let client = crate::ClientId::new(1);
    let other = crate::ClientId::new(2);
    let mut pane = agent_pane();
    pane.update_name("my agent");
    let render = |pane: &mut TerminalPane, client, params| {
        pane.render_frame(client, params, InputMode::Normal)
            .unwrap()
            .map(|(chunks, _)| {
                chunks
                    .into_iter()
                    .filter(|chunk| chunk.y == 0)
                    .flat_map(|chunk| chunk.terminal_characters)
                    .map(|character| character.character)
                    .collect::<String>()
            })
    };
    let normal = render(&mut pane, client, frame_params(true)).unwrap();
    pane.set_pending_agent_escape_clients([client].into_iter().collect());
    let hinted = render(&mut pane, client, frame_params(true)).unwrap();
    assert!(hinted.contains("Esc again"), "{hinted}");
    assert!(hinted.contains("my agent"), "{hinted}");
    assert!(!render(&mut pane, other, frame_params(true))
        .unwrap()
        .contains("Esc again"));
    pane.set_pending_agent_escape_clients(Default::default());
    assert_eq!(
        render(&mut pane, client, frame_params(true)).unwrap(),
        normal
    );

    pane.set_pending_agent_escape_clients([client].into_iter().collect());
    for blank in [false, true] {
        let mut params = frame_params(true);
        params.omit_title = !blank;
        params.blank_title = blank;
        let hidden = render(&mut pane, client, params).unwrap();
        assert!(!hidden.contains("Esc again"));
        assert!(!hidden.contains("my agent"));
    }
    for cols in [5, 12, 20] {
        let mut geom = pane.current_geom();
        geom.cols.set_inner(cols);
        pane.set_geom(geom);
        let narrow = render(&mut pane, client, frame_params(true)).unwrap();
        assert_eq!(narrow.width(), cols, "{narrow}");
        if cols > 8 {
            assert!(
                narrow.contains("[..]") || narrow.contains("[...]"),
                "{narrow}"
            );
        }
    }
    pane.set_borderless(true);
    assert!(render(&mut pane, client, frame_params(true)).is_none());
}

#[test]
fn recovered_codex_palette_follows_themes_without_repeating_queries() {
    use yinz_terminal::panes::terminal_character::AnsiCode;
    use yinz_types::agent::theme::*;
    let mut pane = agent_pane_with_run(Some(super::Run::Command(RunCommand {
        agent_theme: Some(AgentThemeLaunch {
            launch_id: "codex-run".into(),
            snapshot: AgentThemeSnapshot {
                version: SNAPSHOT_VERSION,
                provider: "codex".into(),
                provider_version: "0.162.1".into(),
                theme: "automatic".into(),
                provenance: vec![],
                availability: Availability::Available,
                definitions: vec![],
                collisions: vec![],
            },
        }),
        ..Default::default()
    })));
    pane.handle_pty_bytes(b"\x1b]10;?\x07".to_vec());
    let query = pane.drain_forwarded_queries().remove(0);
    pane.record_terminal_color_reply(&query, b"\x1b]10;rgb:cd/ce/cf\x07");
    pane.handle_pty_bytes(b"\x1b]11;?\x07".to_vec());
    let query = pane.drain_forwarded_queries().remove(0);
    pane.record_terminal_color_reply(&query, b"\x1b]11;rgb:19/23/30\x07");
    let learned = pane.drain_agent_theme_updates().pop().unwrap();
    // Adoption passes the supervisor's command to TerminalPane::new. It has
    // no serialized grid or startup replies from the still-running process.
    let mut adopted = agent_pane_with_run(Some(super::Run::Command(RunCommand {
        agent_theme: Some(AgentThemeLaunch {
            launch_id: learned.launch_id,
            snapshot: learned.snapshot.unwrap(),
        }),
        ..Default::default()
    })));
    adopted.handle_pty_bytes(b"\x1b[48;2;52;61;72mC\x1b[38;2;57;59;62;48;2;99;168;248mS".to_vec());
    for (fg, bg, surface) in [
        ((205, 206, 207), (25, 35, 48), (39, 49, 61)),
        ((20, 20, 20), (255, 255, 255), (236, 236, 236)),
    ] {
        let mut style = Style::default();
        style.colors.text_unselected.base = PaletteColor::Rgb(fg);
        style.colors.text_unselected.background = PaletteColor::Rgb(bg);
        style.colors.text_selected.base = PaletteColor::Rgb(fg);
        style.colors.text_selected.background = PaletteColor::Rgb(bg);
        adopted.grid.update_theme(style.colors);
        let chunks = adopted.grid.render(0, 0, &style).unwrap().unwrap().0;
        let mut found = 0;
        for chunk in chunks {
            for cell in &chunk.terminal_characters {
                let mapped = chunk.application_styles(*cell.styles);
                match cell.character {
                    'C' => {
                        assert_eq!(mapped.background, Some(AnsiCode::RgbCode(surface)));
                        found += 1;
                    },
                    'S' => {
                        assert_eq!(mapped.foreground, Some(AnsiCode::RgbCode(fg)));
                        assert_eq!(mapped.background, Some(AnsiCode::RgbCode(bg)));
                        found += 1;
                    },
                    _ => {},
                }
            }
        }
        assert_eq!(found, 2);
    }
    assert!(adopted.drain_agent_theme_updates().is_empty());
    assert!(adopted.drain_forwarded_queries().is_empty());
}

#[test]
fn agent_exit_notification_preserves_palette_for_pending_pty_output() {
    use yinz_terminal::panes::terminal_character::AnsiCode;
    use yinz_types::agent::theme::*;

    let mut pane = agent_pane();
    let mut style = Style::default();
    style.colors.exit_code_success.base = PaletteColor::Rgb((11, 22, 33));
    pane.grid.update_theme(style.colors);
    let snapshot = AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "claude".into(),
        provider_version: "fixture".into(),
        theme: "fixture".into(),
        provenance: vec![],
        availability: Availability::Available,
        collisions: vec![],
        definitions: vec![Definition {
            token: "status".into(),
            channel: Channel::Foreground,
            color: SourceColor::Index(2),
            pair: None,
            target: Some(Role::Success.into()),
        }],
    };
    pane.grid.apply_agent_theme_event(AgentThemeEvent {
        version: EVENT_VERSION,
        launch_id: "exiting".into(),
        snapshot: Some(snapshot.clone()),
    });
    pane.hold(
        Some(0),
        false,
        RunCommand {
            agent_theme: Some(AgentThemeLaunch {
                launch_id: "exiting".into(),
                snapshot,
            }),
            ..Default::default()
        },
    );
    pane.handle_pty_bytes(b"\x1b[32mL".to_vec());
    pane.grid.apply_agent_theme_event(AgentThemeEvent {
        version: EVENT_VERSION,
        launch_id: "exiting".into(),
        snapshot: None,
    });
    let chunks = pane.grid.render(0, 0, &style).unwrap().unwrap().0;
    let chunk = chunks
        .iter()
        .find(|c| c.terminal_characters.iter().any(|c| c.character == 'L'))
        .unwrap();
    let character = chunk
        .terminal_characters
        .iter()
        .find(|c| c.character == 'L')
        .unwrap();
    assert_eq!(
        chunk.application_styles(*character.styles).foreground,
        Some(AnsiCode::RgbCode((11, 22, 33)))
    );
    let (_, metadata) = pane.serialize_handoff(None, usize::MAX).unwrap();
    assert!(metadata.unwrap().active.is_none());
    let Some(super::Run::Command(command)) = &pane.invoked_with else {
        panic!("held command missing")
    };
    assert!(command.agent_theme.is_none());
}

#[test]
fn agent_restart_is_fresh_and_keeps_command_metadata() {
    let mut pane = agent_pane();
    pane.set_agent_info(Some(AgentInfo {
        kind: "codex".into(),
        state: AgentState::Busy,
        source: AgentSource::Reported,
        session_id: Some("old-session".into()),
        run_id: Some("old-run".into()),
        detail: Some("old task".into()),
        waiting_on: Some("old question".into()),
        wait_needs_a_person: true,
        cwd: Some("/work".into()),
    }));
    let command = RunCommand {
        command: "codex".into(),
        args: vec!["--model".into(), "chosen".into(), "original prompt".into()],
        cwd: Some("/work".into()),
        target: yinz_types::command::PaneTarget::Ssh {
            host: "build-box".into(),
        },
        ..Default::default()
    };
    pane.hold(None, true, command.clone());
    assert_eq!(
        pane.rerun(),
        Some(command.clone()),
        "first held launch retains its prompt"
    );
    let mut expected = command.clone();
    expected.args.pop();
    let mut held = command;
    for _ in 0..2 {
        pane.hold(Some(0), false, held);
        held = pane.rerun().unwrap();
        assert_eq!(held, expected);
        assert_eq!(
            pane.invoked_with,
            Some(super::Run::Command(expected.clone()))
        );
        let agent = pane.agent_info().unwrap();
        assert_eq!(agent.state, AgentState::Busy);
        assert_eq!(agent.cwd.as_deref(), Some("/work"));
        assert!(agent.session_id.is_none() && agent.run_id.is_none());
        assert!(agent.detail.is_none() && agent.waiting_on.is_none());
        assert!(!agent.wait_needs_a_person);
    }
}

#[test]
fn same_command_fresh_replacement_clears_remembered_agent_session() {
    use yinz_types::agent::control::AgentRestartSession;
    use yinz_types::data::{OriginatingPlugin, AGENT_PANE_CONTEXT};

    for plugin_marker in [false, true] {
        let command = if plugin_marker {
            RunCommand {
                command: "claude".into(),
                originating_plugin: Some(OriginatingPlugin::new(
                    yinz_types::internal_ids::PluginId::new(2),
                    ClientId::new(1),
                    std::collections::BTreeMap::from([(
                        AGENT_PANE_CONTEXT.into(),
                        "claude".into(),
                    )]),
                )),
                ..Default::default()
            }
        } else {
            RunCommand {
                command: "env".into(),
                args: vec!["YINZ_AGENT_PANE=1".into(), "claude".into()],
                ..Default::default()
            }
        };
        let mut pane = agent_pane_with_run(Some(super::Run::Command(command.clone())));
        let mut reported = pane.agent_info().unwrap().clone();
        reported.source = AgentSource::Reported;
        reported.session_id = Some("old".into());
        pane.set_agent_info(Some(reported.clone()));
        pane.hold(Some(0), false, command.clone());
        assert_eq!(pane.last_agent_session_id(), Some("old"));
        pane.set_last_hook_at_ms(123);
        assert_eq!(
            pane.respawn(Some(command.clone()), false),
            Some(command.clone())
        );
        assert_eq!(pane.agent_info().unwrap().source, AgentSource::Spawned);
        assert_eq!(pane.agent_info().unwrap().session_id, None);
        assert_eq!(pane.last_agent_session_id(), None);
        assert_eq!(pane.last_hook_at_ms(), None);

        pane.hold(Some(1), false, command);
        assert_eq!(
            pane.agent_restart_command(AgentRestartSession::Resume, None)
                .unwrap_err(),
            "agent pane has no remembered session id"
        );
        reported.session_id = Some("new".into());
        pane.set_agent_info(Some(reported));
        assert_eq!(pane.last_agent_session_id(), Some("new"));
        let resumed = pane
            .agent_restart_command(AgentRestartSession::Resume, None)
            .unwrap();
        assert!(resumed.args.ends_with(&["--resume".into(), "new".into()]));
    }
}

#[test]
fn restart_refusal_keeps_held_state_and_explicit_replacements_work() {
    let mut pane = agent_pane();
    pane.record_agent_input(false);
    pane.set_agent_info(Some(AgentInfo {
        kind: "codex".into(),
        state: AgentState::Busy,
        source: AgentSource::Reported,
        session_id: Some("old-session".into()),
        run_id: Some("old-run".into()),
        detail: Some("old task".into()),
        waiting_on: Some("old question".into()),
        wait_needs_a_person: true,
        cwd: Some("/work".into()),
    }));
    let command = RunCommand {
        command: "codex".into(),
        args: vec!["--unknown".into(), "prompt".into()],
        ..Default::default()
    };
    pane.hold(Some(7), false, command.clone());
    let held = pane.is_held.clone();
    let agent = pane.agent.clone();
    let info = crate::pane::pane_info_for_pane(&pane.pid(), &pane, &HashMap::new());
    assert!(info.restart_error.as_deref().unwrap().contains("--unknown"));
    assert_eq!(pane.is_held, held);
    assert_eq!(pane.agent, agent);
    assert!(pane.rerun().is_none());
    assert_eq!(pane.agent_input_revision(), 1);
    assert_eq!(pane.agent, agent);
    assert_eq!(pane.restart_error(), info.restart_error);
    assert_eq!(pane.is_held, held);
    assert_eq!(pane.exit_status(), Some(7));
    assert!(pane.pane_frame_color_override.is_some());
    assert_eq!(
        pane.respawn(Some(command.clone()), false),
        Some(command.clone())
    );
    assert_eq!(pane.agent_input_revision(), 0);
    pane.was_an_agent = false;
    pane.agent = None;
    pane.hold(Some(0), false, command.clone());
    assert_eq!(pane.rerun(), Some(command), "ordinary reruns are unchanged");
}

#[test]
fn respawn_resets_input_revision_only_when_the_restart_succeeds() {
    for keep_scrollback in [false, true] {
        for replace in [false, true] {
            let mut pane = agent_pane();
            let command = RunCommand {
                command: "codex".into(),
                ..Default::default()
            };
            let replacement = replace.then(|| command.clone());
            assert!(pane.agent_input_at().is_none());
            let before = std::time::Instant::now();
            pane.record_agent_input(true);
            pane.record_agent_input(false);
            assert_eq!(pane.agent_enter_revision(), 1);
            let input_at = pane.agent_input_at().unwrap();
            assert!(input_at >= before);
            assert!(pane.respawn(replacement.clone(), keep_scrollback).is_none());
            assert_eq!(pane.agent_input_revision(), 2);
            assert_eq!(pane.agent_enter_revision(), 1);

            pane.hold(Some(0), false, command.clone());
            pane.set_warp_pending(true);
            assert!(pane.respawn(replacement.clone(), keep_scrollback).is_none());
            assert_eq!(pane.agent_input_revision(), 2);
            assert_eq!(pane.agent_enter_revision(), 1);
            pane.set_warp_pending(false);
            assert_eq!(pane.agent_input_at(), Some(input_at));
            assert_eq!(pane.respawn(replacement, keep_scrollback), Some(command));
            assert_eq!(pane.agent_input_revision(), 0);
            assert!(pane.agent_input_at().is_none());
            assert_eq!(pane.agent_enter_revision(), 0);
        }
    }
}

#[test]
fn dragging_a_selection_past_the_edge_scrolls_further_the_further_it_goes() {
    // One line per interval regardless of the pointer's distance made selecting a long
    // scrollback take seconds; the overshoot now sets the step, capped at ten.
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(40);
    fake_win_size.rows.set_inner(20);
    let mut terminal_pane = TerminalPane::new(
        crate::TerminalId::new(1),
        fake_win_size,
        Style::default(),
        0,
        String::new(),
        Rc::new(RefCell::new(LinkHandler::new())),
        Rc::new(RefCell::new(None)),
        Rc::new(RefCell::new(SixelImageStore::default())),
        Rc::new(RefCell::new(KittyImageStore::default())),
        Rc::new(RefCell::new(Palette::default())),
        Rc::new(RefCell::new(HashMap::new())),
        None,
        None,
        false,
        true,
        true,
        true,
        false,
        None,
    );
    let mut text = String::new();
    for i in 0..60 {
        writeln!(&mut text, "\rline {}", i + 1).unwrap();
    }
    terminal_pane.handle_pty_bytes(text.into_bytes());
    let client_id = crate::ClientId::new(1);
    let interval_elapsed = || std::time::Instant::now() - std::time::Duration::from_millis(50);
    let lines_scrolled_up = |pane: &TerminalPane| pane.grid.scrollback_position_and_length().0;
    terminal_pane.start_selection(&yinz_types::position::Position::new(5, 0), client_id);

    terminal_pane.selection_scrolled_at = interval_elapsed();
    terminal_pane.update_selection(&yinz_types::position::Position::new(-8, 0), client_id);
    assert_eq!(
        lines_scrolled_up(&terminal_pane),
        8,
        "eight lines above the pane: eight up"
    );

    terminal_pane.selection_scrolled_at = interval_elapsed();
    terminal_pane.update_selection(&yinz_types::position::Position::new(-50, 0), client_id);
    assert_eq!(
        lines_scrolled_up(&terminal_pane),
        18,
        "capped at ten per step"
    );

    terminal_pane.selection_scrolled_at = interval_elapsed();
    terminal_pane.update_selection(&yinz_types::position::Position::new(22, 0), client_id);
    assert_eq!(
        lines_scrolled_up(&terminal_pane),
        15,
        "three lines below the pane: three down"
    );
}

#[test]
fn stacked_pane_right_boundaries_stop_at_viewport_top() {
    use crate::ui::boundaries::Boundaries;
    use yinz_types::pane_size::{Dimension, Viewport};

    for (x, y) in [(0, 0), (4, 3)] {
        let viewport = Viewport {
            x,
            y,
            cols: 100,
            rows: 30,
        };
        for (expanded, expected_edges) in [
            (None, ["││││┘", "┐││││┘"]),
            (Some(0), ["┐│││┤┤┘", "┐┤│││┤┤┘"]),
            (Some(1), ["┐┤│││┤┘", "┐┤┤│││┤┘"]),
            (Some(2), ["┐┤┤│││┘", "┐┤┤┤│││┘"]),
        ] {
            for (y_offset, expected_edge) in [0, 3].into_iter().zip(expected_edges) {
                let mut boundaries = Boundaries::new(viewport);
                let mut pane_y = y + y_offset;
                let pane_count = if expanded.is_some() { 3 } else { 1 };
                for index in 0..pane_count {
                    let collapsed = expanded.is_some_and(|expanded| index != expanded);
                    let under = expanded.is_some_and(|expanded| index > expanded);
                    let mut pane = agent_pane();
                    let mut geom = pane.current_geom();
                    geom.x = x + 2;
                    geom.y = pane_y;
                    geom.cols.set_inner(10);
                    geom.stacked = expanded.map(|_| 0);
                    if collapsed {
                        geom.rows = Dimension::fixed(1);
                    } else {
                        geom.rows.set_inner(5);
                    }
                    pane.set_geom(geom);
                    pane.set_content_offset(Offset {
                        top: usize::from(expanded.is_some() && !collapsed),
                        bottom: 1,
                        right: 1,
                        left: 0,
                    });
                    boundaries.add_rect(
                        &pane,
                        None,
                        None,
                        index == 0,
                        index == pane_count - 1,
                        under,
                    );
                    pane_y += geom.rows.as_usize();
                }
                let mut right_edge: Vec<_> = boundaries
                    .render(None)
                    .unwrap()
                    .into_iter()
                    .filter(|chunk| chunk.x == x + 11)
                    .map(|chunk| (chunk.y, chunk.terminal_characters[0].character))
                    .collect();
                right_edge.sort_unstable();
                let first_row = y + y_offset.saturating_sub(1);
                let expected: Vec<_> = expected_edge
                    .chars()
                    .enumerate()
                    .map(|(row, glyph)| (first_row + row, glyph))
                    .collect();
                assert_eq!(
                    right_edge, expected,
                    "{viewport:?}, {expanded:?}, {y_offset}"
                );
            }
        }
    }
}

#[test]
fn frame_and_boundary_backgrounds_match_terminal_themes_and_ignore_application_overrides() {
    use crate::panes::terminal_character::AnsiCode;
    use crate::ui::boundaries::Boundaries;
    use yinz_types::pane_size::Viewport;

    for (name, source) in [
        (
            "tokyo-night",
            include_str!("../../../../yinz-config/assets/themes/tokyo-night.kdl"),
        ),
        (
            "dracula",
            include_str!("../../../../yinz-config/assets/themes/dracula.kdl"),
        ),
        (
            "molokai-dark",
            include_str!("../../../../yinz-config/assets/themes/molokai-dark.kdl"),
        ),
    ] {
        let themes = yinz_config::theme::Themes::from_string(source, false).unwrap();
        let theme = themes.get_theme(name).unwrap();
        let style = Style {
            terminal: theme.terminal,
            colors: theme.palette,
            ..Style::default()
        };
        let terminal = theme.terminal.unwrap();
        let background = Some(AnsiCode::RgbCode(terminal.background));
        let mut pane = agent_pane();
        let host_colors = pane.host_colors();
        pane.update_theme(theme.palette);
        pane.update_terminal_theme(theme.terminal);
        pane.set_content_offset(Offset::frame(1));
        pane.handle_pty_bytes(b"content".to_vec());
        let (chunks, _, _, _) = pane.render(None).unwrap().unwrap();
        assert!(!chunks.is_empty(), "{name}");
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.pane_default_bg == background),
            "{name}"
        );

        for application_override in [false, true] {
            if application_override {
                pane.handle_pty_bytes(b"\x1b]11;#010203\x07".to_vec());
                let (chunks, _, _, _) = pane.render(None).unwrap().unwrap();
                assert!(!chunks.is_empty());
                assert!(chunks
                    .iter()
                    .all(|chunk| chunk.pane_default_bg == Some(AnsiCode::RgbCode((1, 2, 3)))));
                pane.invalidate_frame(ClientId::new(1));
            }
            assert_eq!(pane.host_colors(), host_colors);
            let mut params = frame_params(false);
            params.style = style;
            params.content_offset = pane.get_content_offset();
            let (frame, _) = pane
                .render_frame(ClientId::new(1), params, InputMode::Normal)
                .unwrap()
                .unwrap();
            assert!(!frame.is_empty());
            assert!(
                frame
                    .iter()
                    .all(|chunk| chunk.pane_default_bg == background),
                "{name}"
            );

            let mut boundaries = Boundaries::new(Viewport {
                x: 0,
                y: 0,
                cols: 100,
                rows: 30,
            });
            boundaries.add_rect(&pane, None, style.chrome_colors(), true, true, false);
            let boundary = boundaries.render(None).unwrap();
            assert!(!boundary.is_empty());
            assert!(
                boundary
                    .iter()
                    .flat_map(|chunk| &chunk.terminal_characters)
                    .all(|character| character.styles.background == background),
                "{name}"
            );
        }
    }
}

#[test]
fn terminal_frames_keep_the_calculated_fade_color() {
    use crate::panes::terminal_character::AnsiCode;

    let mut pane = agent_pane();
    pane.flash_pane_frame_color_override(std::time::Instant::now());
    for color in [Some(PaletteColor::Rgb((30, 40, 50))), None] {
        let mut params = frame_params(false);
        params.color = color;
        let (frame, _) = pane
            .render_frame(ClientId::new(1), params, InputMode::Normal)
            .unwrap()
            .unwrap();
        assert!(!frame.is_empty());
        assert!(frame
            .iter()
            .flat_map(|chunk| &chunk.terminal_characters)
            .all(|character| character.styles.foreground
                == Some(color.map(Into::into).unwrap_or(AnsiCode::Reset))));
    }
}

#[test]
fn changing_provider_does_not_inherit_another_providers_session() {
    for kind in ["claude", "codex", "pi", "muse", "agy"] {
        for clear_record in [false, true] {
            let mut pane = agent_pane();
            let mut previous = AgentInfo {
                kind: if kind == "claude" { "codex" } else { "claude" }.into(),
                state: AgentState::Busy,
                source: AgentSource::Reported,
                session_id: Some("old-session".into()),
                run_id: None,
                detail: None,
                waiting_on: None,
                wait_needs_a_person: false,
                cwd: None,
            };
            pane.set_agent_info(Some(previous.clone()));
            if clear_record {
                pane.set_agent_info(None);
            }
            previous.kind = kind.into();
            previous.session_id = None;
            pane.set_agent_info(Some(previous));
            assert_eq!(pane.last_agent_session_id(), None, "{kind}/{clear_record}");
        }
    }
}

#[test]
fn reusing_an_agent_pane_for_a_non_agent_clears_its_session_memory() {
    let mut pane = agent_pane_with_run(Some(super::Run::Command(RunCommand {
        command: "env".into(),
        args: vec!["YINZ_AGENT_PANE=1".into(), "claude".into()],
        ..Default::default()
    })));
    let mut reported = pane.agent_info().unwrap().clone();
    reported.session_id = Some("old-session".into());
    pane.set_agent_info(Some(reported));
    pane.set_agent_info(None);
    pane.hold(
        Some(0),
        false,
        RunCommand {
            command: "claude".into(),
            ..Default::default()
        },
    );
    pane.respawn(
        Some(RunCommand {
            command: "/usr/bin/true".into(),
            ..Default::default()
        }),
        false,
    )
    .unwrap();
    let info = crate::pane::pane_info_for_pane(&pane.pid(), &pane, &HashMap::new());
    assert!(info.agent.is_none());
    assert!(info.last_agent_session_id.is_none());
    assert!(!pane.was_an_agent());
}

#[test]
fn focus_emoji_follows_focus_without_renaming_the_pane() {
    let mut pane = agent_pane();
    pane.update_name("my pane");
    for focused in [false, true, false] {
        let rendered: String = pane
            .render_frame(ClientId::new(1), frame_params(focused), InputMode::Normal)
            .unwrap()
            .unwrap()
            .0
            .into_iter()
            .flat_map(|chunk| chunk.terminal_characters)
            .map(|character| character.character)
            .collect();
        assert_eq!(rendered.contains('👇'), focused);
        assert_eq!(pane.display_title(), "my pane");
        assert!(pane
            .render_frame(ClientId::new(1), frame_params(focused), InputMode::Normal)
            .unwrap()
            .is_none());
    }
}
