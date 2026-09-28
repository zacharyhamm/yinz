static BANNER: &str = "
██╗  ██╗██╗    ███████╗██████╗  ██████╗ ███╗   ███╗    ██╗   ██╗██╗███╗   ██╗███████╗██╗
██║  ██║██║    ██╔════╝██╔══██╗██╔═══██╗████╗ ████║    ╚██╗ ██╔╝██║████╗  ██║╚══███╔╝██║
███████║██║    █████╗  ██████╔╝██║   ██║██╔████╔██║     ╚████╔╝ ██║██╔██╗ ██║  ███╔╝ ██║
██╔══██║██║    ██╔══╝  ██╔══██╗██║   ██║██║╚██╔╝██║      ╚██╔╝  ██║██║╚██╗██║ ███╔╝  ╚═╝
██║  ██║██║    ██║     ██║  ██║╚██████╔╝██║ ╚═╝ ██║       ██║   ██║██║ ╚████║███████╗██╗
╚═╝  ╚═╝╚═╝    ╚═╝     ╚═╝  ╚═╝ ╚═════╝ ╚═╝     ╚═╝       ╚═╝   ╚═╝╚═╝  ╚═══╝╚══════╝╚═╝
";

static SMALL_BANNER: &str = "
██╗  ██╗██╗    ██╗
██║  ██║██║    ██║
███████║██║    ██║
██╔══██║██║    ╚═╝
██║  ██║██║    ██╗
╚═╝  ╚═╝╚═╝    ╚═╝
";

static MEDIUM_BANNER: &str = "
██╗  ██╗██╗    ████████╗██╗  ██╗███████╗██████╗ ███████╗    ██╗
██║  ██║██║    ╚══██╔══╝██║  ██║██╔════╝██╔══██╗██╔════╝    ██║
███████║██║       ██║   ███████║█████╗  ██████╔╝█████╗      ██║
██╔══██║██║       ██║   ██╔══██║██╔══╝  ██╔══██╗██╔══╝      ╚═╝
██║  ██║██║       ██║   ██║  ██║███████╗██║  ██║███████╗    ██╗
╚═╝  ╚═╝╚═╝       ╚═╝   ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝    ╚═╝                 
";

pub fn render_banner(x: usize, y: usize, rows: usize, cols: usize) {
    if rows >= 8 {
        if cols > 100 {
            println!("\u{1b}[{}H", y + rows.saturating_sub(8) / 2);
            // BANNER is 88 columns wide; center it within the 101-column main menu.
            for line in BANNER.lines() {
                println!("\u{1b}[{}C{}", x.saturating_sub(1) + (101 - 88) / 2, line);
            }
        } else if cols > 63 {
            println!("\u{1b}[{}H", y + rows.saturating_sub(8) / 2);
            let x = (cols.saturating_sub(63) as f64 / 2.0) as usize;
            for line in MEDIUM_BANNER.lines() {
                println!("\u{1b}[{}C{}", x, line);
            }
        } else {
            println!("\u{1b}[{}H", y + rows.saturating_sub(8) / 2);
            let x = (cols.saturating_sub(18) as f64 / 2.0) as usize;
            for line in SMALL_BANNER.lines() {
                println!("\u{1b}[{}C{}", x, line);
            }
        }
    } else if rows > 2 {
        println!(
            "\u{1b}[{};{}H\u{1b}[1mHi from Yinz!",
            (y + rows / 2) + 1,
            (x + cols.saturating_sub(13) / 2).saturating_sub(1)
        );
    }
}

pub fn render_welcome_boundaries(rows: usize, cols: usize) {
    let width_of_main_menu = std::cmp::min(cols, 101);
    let left_boundary_x = (cols.saturating_sub(width_of_main_menu) as f64 / 2.0).floor() as usize;
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
