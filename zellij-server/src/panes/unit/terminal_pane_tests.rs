use super::super::TerminalPane;
use crate::panes::kitty_graphics::KittyImageStore;
use crate::panes::sixel::SixelImageStore;
use crate::panes::LinkHandler;
use crate::tab::Pane;
use crate::ui::pane_boundaries_frame::FrameParams;
use insta::assert_snapshot;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use zellij_types::internal_ids::ClientId;
use zellij_types::position::Position;
use zellij_types::style::{Palette, Style};
use zellij_types::{
    agent::{AgentInfo, AgentSource, AgentState},
    pane_size::{Offset, PaneGeom, SizeInPixels},
};
use {zellij_types::data::InputMode, zellij_types::input::command::RunCommand};

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
    let fake_client_id = ClientId(1);
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
        crate::TerminalId(pid),
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
    terminal_pane.scroll_up(10, fake_client_id);
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.scroll_down(3, fake_client_id);
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
        crate::TerminalId(pid),
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
        crate::TerminalId(pid),
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
        crate::TerminalId(pid),
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
    let fake_client_id = ClientId(1);
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
        crate::TerminalId(pid),
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
    terminal_pane.scroll_up(10, fake_client_id);
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.scroll_down(3, fake_client_id);
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
    terminal_pane.clear_scroll();
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn multiple_sixel_images_in_pane() {
    let fake_client_id = ClientId(1);
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
        crate::TerminalId(pid),
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
    terminal_pane.scroll_up(20, fake_client_id); // scroll up to see both images
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn resizing_pane_with_sixel_images() {
    // here we test, for example, that sixel images don't wrap with other lines
    let fake_client_id = ClientId(1);
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
        crate::TerminalId(pid),
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
    terminal_pane.scroll_up(20, fake_client_id); // scroll up to see both images
    assert_snapshot!(format!("{:?}", terminal_pane.grid));
}

#[test]
pub fn changing_character_cell_size_with_sixel_images() {
    let fake_client_id = ClientId(1);
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
        crate::TerminalId(pid),
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
    terminal_pane.scroll_up(10, fake_client_id); // scroll up to see both images
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
        crate::TerminalId(pid),
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
        crate::TerminalId(pid),
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
        crate::TerminalId(pid),
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
        crate::TerminalId(1),
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
/// summary hocus-agentd writes there.
#[test]
pub fn a_title_prefix_survives_every_kind_of_title() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_title_prefix(Some("zellij:topic".to_owned()));
    assert_eq!(terminal_pane.display_title(), "zellij:topic");
    terminal_pane.grid.title = Some("running the tests".to_owned());
    assert_eq!(
        terminal_pane.display_title(),
        "zellij:topic: running the tests"
    );
    terminal_pane.rename(b"custom-name".to_vec());
    assert_eq!(terminal_pane.display_title(), "zellij:topic: custom-name");
}

/// And the prefix does not become a name either: `current_title` is what the pane
/// manifest carries, and agent detection reads that title. A prefix that leaked
/// into it would let the daemon re-detect its own decoration.
#[test]
pub fn a_title_prefix_stays_out_of_the_raw_title() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    let before = terminal_pane.current_title();
    terminal_pane.set_title_prefix(Some("zellij:topic".to_owned()));
    assert_eq!(terminal_pane.current_title(), before);
    assert_eq!(terminal_pane.custom_title(), None);
    assert!(!terminal_pane.has_explicit_title());
}

#[test]
pub fn a_title_prefix_can_be_dropped() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.grid.title = Some("running the tests".to_owned());
    terminal_pane.set_title_prefix(Some("zellij:topic".to_owned()));
    terminal_pane.set_title_prefix(None);
    assert_eq!(terminal_pane.display_title(), "running the tests");
}

/// Both decorations at once, in the order they answer "where is this pane": the
/// machine, then the checkout on it.
#[test]
pub fn a_title_prefix_follows_the_hostname() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_remote_host(Some("myhost".to_owned()));
    terminal_pane.set_title_prefix(Some("zellij:topic".to_owned()));
    assert_eq!(terminal_pane.display_title(), "myhost zellij:topic");
    terminal_pane.grid.title = Some("running the tests".to_owned());
    assert_eq!(
        terminal_pane.display_title(),
        "myhost zellij:topic: running the tests"
    );
}

/// The frame is the one place the pane's id is written, so it leads whatever the
/// title turns out to be — placeholder, OSC title, or an address and a title.
#[test]
pub fn a_framed_title_leads_with_the_pane_id() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    assert_eq!(terminal_pane.framed_title(), "[1] Pane #0");
    terminal_pane.set_title_prefix(Some("zellij:topic".to_owned()));
    assert_eq!(terminal_pane.framed_title(), "[1] zellij:topic");
    terminal_pane.grid.title = Some("my-shell".to_owned());
    assert_eq!(terminal_pane.framed_title(), "[1] zellij:topic: my-shell");
}

/// `display_title` is also a tab's name when the tab holds one pane, so the id
/// stays out of it.
#[test]
pub fn a_pane_id_stays_out_of_the_display_title() {
    let mut terminal_pane = make_terminal_pane_for_bell();
    terminal_pane.set_title_prefix(Some("zellij:topic".to_owned()));
    assert_eq!(terminal_pane.display_title(), "zellij:topic");
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
        crate::TerminalId(pid),
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

fn create_guest_modal_pane() -> TerminalPane {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(80);
    fake_win_size.rows.set_inner(24);
    let sixel_image_store = Rc::new(RefCell::new(SixelImageStore::default()));
    let terminal_emulator_colors = Rc::new(RefCell::new(Palette::default()));
    let terminal_emulator_color_codes = Rc::new(RefCell::new(HashMap::new()));
    TerminalPane::new(
        crate::TerminalId(1),
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
    bare_key: zellij_types::keys::BareKey,
    client_id: ClientId,
) -> Option<crate::tab::AdjustedInput> {
    let key = zellij_types::keys::KeyWithModifier::new(bare_key);
    pane.adjust_input_to_terminal(&Some(key), vec![], false, Some(client_id))
}

#[test]
fn an_ordinary_held_command_still_drops_to_a_shell_on_escape() {
    use crate::tab::AdjustedInput;
    use zellij_types::keys::BareKey;

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
        press_key(&mut pane, BareKey::Esc, crate::ClientId(1)),
        Some(AdjustedInput::DropToShellInThisPane { .. })
    ));
}

#[test]
pub fn guest_modal_navigation_wraps() {
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);
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
    use {zellij_types::data::BareKey, zellij_types::data::KeyWithModifier};
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);
    pane.set_guest_modal(&[client_id]);
    press_key(&mut pane, BareKey::Tab, client_id);
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
    let key = KeyWithModifier::new(BareKey::Tab).with_shift_modifier();
    pane.adjust_input_to_terminal(&Some(key), vec![], false, Some(client_id));
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
}

#[test]
pub fn guest_modal_enter_confirms_selection() {
    use crate::tab::AdjustedInput;
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);

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
    use crate::tab::AdjustedInput;
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);

    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Char('1'), client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalZoom)));

    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Char('2'), client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalDescend)));
}

#[test]
pub fn guest_modal_esc_descends() {
    use crate::tab::AdjustedInput;
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);
    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Esc, client_id);
    assert!(matches!(outcome, Some(AdjustedInput::GuestModalDescend)));
}

#[test]
pub fn guest_modal_swallows_other_input() {
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);
    pane.set_guest_modal(&[client_id]);
    let outcome = press_key(&mut pane, BareKey::Char('x'), client_id);
    assert!(outcome.is_none());
    assert_eq!(pane.guest_modal_selection(client_id), Some(0));
}

#[test]
pub fn guest_modal_is_per_client() {
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_a = 1;
    let client_b = 2;
    pane.set_guest_modal(&[crate::ClientId(client_a), crate::ClientId(client_b)]);
    press_key(&mut pane, BareKey::Down, crate::ClientId(client_a));
    assert_eq!(
        pane.guest_modal_selection(crate::ClientId(client_a)),
        Some(1)
    );
    assert_eq!(
        pane.guest_modal_selection(crate::ClientId(client_b)),
        Some(0)
    );
}

#[test]
pub fn guest_modal_no_modal_passes_input_through() {
    use zellij_types::keys::BareKey;
    let mut pane = create_guest_modal_pane();
    let client_id = ClientId(1);
    let outcome = press_key(&mut pane, BareKey::Char('x'), client_id);
    assert!(matches!(
        outcome,
        Some(crate::tab::AdjustedInput::WriteBytesToTerminal(_))
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

fn make_pane_for_ingest_with_cell_size(
    character_cell_size: Rc<RefCell<Option<SizeInPixels>>>,
) -> TerminalPane {
    let mut fake_win_size = PaneGeom::default();
    fake_win_size.cols.set_inner(121);
    fake_win_size.rows.set_inner(20);
    TerminalPane::new(
        crate::TerminalId(1),
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
    // The batch loop advances the index by a whole run at once, and the interceptor's swallow
    // branch then re-queues from `index - 1` - the escape it just consumed. This is the input
    // that puts a batched run between the two: an OSC that has to be forwarded to the host, a run
    // of ordinary bytes, and then a kitty sequence whose opening escape triggers the flush.
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
    use crate::tab::AdjustedInput;
    use zellij_types::keys::BareKey;

    let mut pane = suspended_pane();
    assert!(matches!(
        press_key(&mut pane, BareKey::Enter, crate::ClientId(1)),
        Some(AdjustedInput::ResumeSuspendedProcessInThisPane)
    ));

    let mut pane = suspended_pane();
    assert!(matches!(
        pane.adjust_input_to_terminal(&None, vec![b' '], false, Some(crate::ClientId(1))),
        Some(AdjustedInput::ResumeSuspendedProcessInThisPane)
    ));

    let mut pane = suspended_pane();
    assert!(matches!(
        pane.adjust_input_to_terminal(&None, vec![3], false, Some(crate::ClientId(1))),
        Some(AdjustedInput::CloseThisPane)
    ));

    let mut pane = suspended_pane();
    assert!(
        press_key(&mut pane, BareKey::Char('x'), crate::ClientId(1)).is_none(),
        "an ordinary key must not reach a stopped process"
    );

    // Esc means "drop to shell" for a held pane; a suspended pane has a live
    // process and nothing to drop out of.
    let mut pane = suspended_pane();
    assert!(press_key(&mut pane, BareKey::Esc, crate::ClientId(1)).is_none());
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
    use crate::tab::AdjustedInput;
    use zellij_types::keys::BareKey;

    let mut pane = suspended_pane();
    pane.set_suspended(false);
    assert!(matches!(
        press_key(&mut pane, BareKey::Char('x'), crate::ClientId(1)),
        Some(AdjustedInput::WriteBytesToTerminal(_))
    ));
}

/// The wait's indicator is taken down by the client that is *looking* at the pane, in the
/// frame it draws for them.
///
/// Focus reaches a pane by a dozen routes - tiled, floating, fullscreen, a mouse click, a
/// plugin - and the frame is the one place that knows the answer for certain. The corollary
/// is deliberate: a wait that arrives while the user is already in the pane never blinks at
/// all, which is the same rule the waiting toast keeps.
#[test]
pub fn focusing_a_pane_takes_down_its_waiting_agents_bell() {
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

    // A client the pane is *not* focused for leaves the wait up: it is drawing the bell, not
    // answering it.
    let mut pane = agent_pane();
    assert!(pane.set_agent_info(Some(waiting.clone())));
    assert!(pane.agent_attention_since().is_some());
    pane.render_frame(crate::ClientId(1), frame_params(false), InputMode::Normal)
        .expect("the frame renders");
    assert!(
        pane.agent_attention_since().is_some(),
        "somebody else's frame is not somebody answering"
    );

    // The focused client's is.
    pane.render_frame(crate::ClientId(1), frame_params(true), InputMode::Normal)
        .expect("the frame renders");
    assert!(pane.agent_attention_since().is_none());

    // And a wait that arrives while they are already there never goes up.
    let mut busy = waiting.clone();
    busy.state = AgentState::Busy;
    busy.waiting_on = None;
    busy.wait_needs_a_person = false;
    assert!(pane.set_agent_info(Some(busy)));
    assert!(pane.set_agent_info(Some(waiting)));
    assert!(pane.agent_attention_since().is_some());
    pane.render_frame(crate::ClientId(1), frame_params(true), InputMode::Normal)
        .expect("the frame renders");
    assert!(pane.agent_attention_since().is_none());
}

fn agent_pane() -> TerminalPane {
    let mut geom = PaneGeom::default();
    geom.cols.set_inner(80);
    geom.rows.set_inner(20);
    TerminalPane::new(
        crate::TerminalId(1),
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
        None,
        false, // debug
        true,  // arrow_fonts
        true,  // styled_underlines
        true,  // osc8_hyperlinks
        false, // explicitly_disable_kitty_keyboard_protocol
        None,
    )
}

fn frame_params(is_main_client: bool) -> FrameParams {
    FrameParams {
        focused_client: is_main_client.then_some(crate::ClientId(1)),
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
        mouse_scroll_resize: false,
        dimmed: false,
        guest_choice_indicator: None,
    }
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
        crate::TerminalId(pid),
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
