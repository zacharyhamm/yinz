use unicode_width::UnicodeWidthStr;

/// The welcome screen's main menu never grows wider than this; wider panes center it.
const MAIN_MENU_MAX_WIDTH: usize = 101;

/// The main menu's width in a welcome screen `cols` wide.
pub fn main_menu_width(cols: usize) -> usize {
    cols.min(MAIN_MENU_MAX_WIDTH)
}

/// The zero-based column of the main menu's left border in a welcome screen `cols` wide.
pub fn main_menu_left_border(cols: usize) -> usize {
    cols.saturating_sub(main_menu_width(cols)) / 2
}

const BANNER: &str = "
██╗  ██╗██╗    ███████╗██████╗  ██████╗ ███╗   ███╗    ██╗   ██╗██╗███╗   ██╗███████╗██╗
██║  ██║██║    ██╔════╝██╔══██╗██╔═══██╗████╗ ████║    ╚██╗ ██╔╝██║████╗  ██║╚══███╔╝██║
███████║██║    █████╗  ██████╔╝██║   ██║██╔████╔██║     ╚████╔╝ ██║██╔██╗ ██║  ███╔╝ ██║
██╔══██║██║    ██╔══╝  ██╔══██╗██║   ██║██║╚██╔╝██║      ╚██╔╝  ██║██║╚██╗██║ ███╔╝  ╚═╝
██║  ██║██║    ██║     ██║  ██║╚██████╔╝██║ ╚═╝ ██║       ██║   ██║██║ ╚████║███████╗██╗
╚═╝  ╚═╝╚═╝    ╚═╝     ╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚═╝       ╚═╝   ╚═╝╚═╝  ╚═══╝╚══════╝╚═╝
";

const SMALL_BANNER: &str = "
██╗  ██╗██╗    ██╗
██║  ██║██║    ██║
███████║██║    ██║
██╔══██║██║    ╚═╝
██║  ██║██║    ██╗
╚═╝  ╚═╝╚═╝    ╚═╝
";

const MEDIUM_BANNER: &str = "
██╗  ██╗██╗    ████████╗██╗  ██╗███████╗██████╗ ███████╗    ██╗
██║  ██║██║    ╚══██╔══╝██║  ██║██╔════╝██╔══██╗██╔════╝    ██║
███████║██║       ██║   ███████║█████╗  ██████╔╝█████╗      ██║
██╔══██║██║       ██║   ██╔══██║██╔══╝  ██╔══██╗██╔══╝      ╚═╝
██║  ██║██║       ██║   ██║  ██║███████╗██║  ██║███████╗    ██╗
╚═╝  ╚═╝╚═╝       ╚═╝   ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝    ╚═╝
";

/// Banners from widest to narrowest; `render_banner` draws the first that fits.
const BANNERS: [&str; 3] = [BANNER, MEDIUM_BANNER, SMALL_BANNER];

/// Drawn instead of a banner when the menu leaves too few rows or columns for one.
const GREETING: &str = "Hi from Yinz!";

/// The display width of the widest line of `text`.
fn display_width(text: &str) -> usize {
    text.lines().map(UnicodeWidthStr::width).max().unwrap_or(0)
}

/// The column, counted from the main menu's left border, that centers `text` over the
/// menu's interior, or `None` when a menu `menu_width` wide has no room for it.
fn centered_column(text: &str, menu_width: usize) -> Option<usize> {
    // The left border takes the menu's first column; the right one sits just past its last.
    let interior = menu_width.saturating_sub(1);
    let width = display_width(text);
    (width <= interior).then(|| 1 + (interior - width) / 2)
}

/// The widest banner that fits over a main menu `menu_width` wide, with its column.
fn banner_for(menu_width: usize) -> Option<(&'static str, usize)> {
    BANNERS
        .into_iter()
        .find_map(|art| centered_column(art, menu_width).map(|column| (art, column)))
}

/// Draws a banner above the main menu, or a one-line greeting when it has no room for one.
/// The menu's left border is at zero-based column `left_border` (see `main_menu_left_border`),
/// the menu is `menu_width` wide, and `rows` rows starting at `y` are free above it.
pub fn render_banner(left_border: usize, y: usize, rows: usize, menu_width: usize) {
    for line in banner_lines(left_border, y, rows, menu_width) {
        println!("{line}");
    }
}

/// The lines `render_banner` prints, without their newlines.
fn banner_lines(left_border: usize, y: usize, rows: usize, menu_width: usize) -> Vec<String> {
    if rows >= 8 {
        if let Some((art, column)) = banner_for(menu_width) {
            let start = format!("\u{1b}[{}H", y + rows.saturating_sub(8) / 2);
            // Each line starts at column 0 of its row, so the move is the zero-based column.
            let art = art
                .lines()
                .map(|line| format!("\u{1b}[{}C{}", left_border + column, line));
            return std::iter::once(start).chain(art).collect();
        }
    }
    if rows > 2 {
        if let Some(column) = centered_column(GREETING, menu_width) {
            // The cursor position counts columns from one.
            return vec![format!(
                "\u{1b}[{};{}H\u{1b}[1m{}",
                y + rows / 2 + 1,
                left_border + column + 1,
                GREETING
            )];
        }
    }
    Vec::new()
}

pub fn render_welcome_boundaries(rows: usize, cols: usize) {
    let width_of_main_menu = main_menu_width(cols);
    let left_boundary_x = main_menu_left_border(cols);
    let right_boundary_x = left_boundary_x + width_of_main_menu;
    let y_starting_point = rows.saturating_sub(15) / 2;
    for i in y_starting_point..rows {
        if i == y_starting_point {
            print!("\u{1b}[{};{}H┌", i + 1, left_boundary_x + 1);
            print!(
                "\u{1b}[m\u{1b}[{};{}H┐\u{1b}[K",
                i + 1,
                right_boundary_x + 1
            );
        } else if i == rows.saturating_sub(1) {
            print!("\u{1b}[{};{}H└", i + 1, left_boundary_x + 1);
            print!(
                "\u{1b}[m\u{1b}[{};{}H┘\u{1b}[K",
                i + 1,
                right_boundary_x + 1
            );
        } else {
            print!("\u{1b}[{};{}H│", i + 1, left_boundary_x + 1);
            print!(
                "\u{1b}[m\u{1b}[{};{}H│\u{1b}[K",
                i + 1,
                right_boundary_x + 1
            ); // this includes some
               // ANSI magic to delete
               // everything after this
               // boundary in order to
               // fix some rendering
               // bugs in the legacy
               // components of this
               // plugin
        }
    }
}

#[cfg(test)]
#[path = "unit/welcome_screen.rs"]
mod tests;
