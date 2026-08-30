//! The wand hocus loops on an empty chat: twelve embedded PNG frames drawn with the kitty
//! graphics protocol, and a text fallback for terminals that cannot show one.
//!
//! The protocol has no playback clock - a terminal stores frames but never advances them on its
//! own - so the animation is driven from hocus's own timer chain, one frame per tick, which is
//! the same chain the spinner already runs on.
//!
//! Frames are embedded rather than shipped beside the wasm: `cargo x install` copies only the
//! `.wasm` and prunes the rest of the version directory, so a sidecar file would not survive.
//! See `assets/wand/README.md` for how they are drawn.

use zellij_tile::prelude::*;

/// The frames, in loop order. They are the whole runtime asset.
pub const FRAMES: [&[u8]; 12] = [
    include_bytes!("../assets/wand/frame-00.png"),
    include_bytes!("../assets/wand/frame-01.png"),
    include_bytes!("../assets/wand/frame-02.png"),
    include_bytes!("../assets/wand/frame-03.png"),
    include_bytes!("../assets/wand/frame-04.png"),
    include_bytes!("../assets/wand/frame-05.png"),
    include_bytes!("../assets/wand/frame-06.png"),
    include_bytes!("../assets/wand/frame-07.png"),
    include_bytes!("../assets/wand/frame-08.png"),
    include_bytes!("../assets/wand/frame-09.png"),
    include_bytes!("../assets/wand/frame-10.png"),
    include_bytes!("../assets/wand/frame-11.png"),
];

/// The cell box the image is placed over. The artwork is padded to this aspect and a terminal
/// cell is about half as wide as it is tall, so this shows it undistorted -
/// `gen_frames.py` prints the number to keep these two honest with each other.
pub const WAND_COLS: usize = 10;
pub const WAND_ROWS: usize = 6;

/// Image ids for the frames. Arbitrary, but under `MAX_IMAGE_ID` so the whole id rides in the
/// placeholder cell's foreground colour, and high enough to leave room below for anything else
/// hocus might draw later. Ids are scoped to the pane, so nothing else can collide with them.
const FIRST_IMAGE_ID: u32 = 0x00_b0_00;

/// The smallest pane the image fits in, with a line of prompt under it.
pub const MIN_COLS: usize = WAND_COLS + 4;
pub const MIN_ROWS: usize = WAND_ROWS + 2;

pub fn image_id(frame: usize) -> u32 {
    FIRST_IMAGE_ID + (frame % FRAMES.len()) as u32
}

/// The bytes that draw frame `frame` with its top left corner at `x`, `y`.
///
/// `transmit` carries the pixels of every frame and belongs on the first draw for a client only -
/// it is the expensive part, and once the images are in the pane's store the placeholder cells
/// below refer to them for free. Composed into one string rather than printed piecemeal so the
/// whole frame reaches the pane as a unit.
pub fn frame_bytes(frame: usize, x: usize, y: usize, transmit: bool) -> String {
    let mut bytes = String::new();
    if transmit {
        for (index, png) in FRAMES.iter().enumerate() {
            bytes.push_str(&png_transmit_sequence(image_id(index), png));
            bytes.push_str(&place_image_sequence(image_id(index), WAND_COLS, WAND_ROWS));
        }
    }
    for row in 0..WAND_ROWS {
        // Absolute positioning per row, the way `print_text_with_coordinates` works, so this
        // composes with the rest of the frame.
        bytes.push_str(&format!("\u{1b}[{};{}H", y + row + 1, x + 1));
        bytes.push_str(&image_row(image_id(frame), row, WAND_COLS));
    }
    bytes
}

/// Delete every wand frame and release its pixel data from this client's pane image store.
pub fn delete_bytes() -> String {
    format!(
        "\u{1b}_Ga=d,d=R,q=2,x={},y={}\u{1b}\\",
        image_id(0),
        image_id(FRAMES.len() - 1)
    )
}

/// The wand as text, for a terminal that cannot show the image. Three arcane marks twinkle out
/// of step beside it, on the same tick as the real thing.
pub fn fallback_lines(frame: usize) -> Vec<String> {
    const SPARKS: [char; 5] = ['\u{b7}', '\u{22c6}', '\u{2727}', '\u{2726}', '\u{25cb}'];
    const ART: [&str; 5] = ["   \u{2726}", "  \u{2571}", " \u{2571}", "\u{2571}", ""];
    let mut lines = Vec::with_capacity(ART.len());
    for (row, art) in ART.iter().enumerate() {
        // Each column runs through the sparkle sizes at its own offset, so they twinkle out of
        // step rather than pulsing together.
        let spark = |column: usize| {
            let phase = (frame + column * 3 + row * 5) % (SPARKS.len() * 2);
            if phase < SPARKS.len() {
                SPARKS[phase]
            } else {
                ' '
            }
        };
        lines.push(format!(
            "{}{}{} {} {}",
            art,
            " ".repeat(5usize.saturating_sub(art.chars().count())),
            spark(0),
            spark(1),
            spark(2)
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_frame_carries_the_pixels_and_a_later_one_only_refers_to_them() {
        let first = frame_bytes(0, 0, 0, true);
        let later = frame_bytes(1, 0, 0, false);
        // Every frame's pixels go over on the first draw, so a tick never waits on a transfer.
        assert_eq!(first.matches("a=t,f=100").count(), transmit_commands());
        assert_eq!(later.matches("a=t,f=100").count(), 0);
        assert!(later.len() < first.len() / 10, "{} bytes", later.len());
    }

    /// One `a=t` command per frame - the first chunk of each transmission.
    fn transmit_commands() -> usize {
        FRAMES.len()
    }

    #[test]
    fn every_frame_draws_the_same_block_of_cells_at_the_same_place() {
        assert_eq!((WAND_COLS, WAND_ROWS), (10, 6));
        for frame in 0..FRAMES.len() {
            let bytes = frame_bytes(frame, 3, 2, false);
            assert_eq!(bytes.matches('\u{10EEEE}').count(), WAND_COLS * WAND_ROWS);
            // Top left corner where it was asked for, in 1-based cursor coordinates.
            assert!(bytes.starts_with("\u{1b}[3;4H"), "{:?}", &bytes[..12]);
        }
    }

    #[test]
    fn each_frame_refers_to_its_own_image() {
        let ids: std::collections::BTreeSet<u32> = (0..FRAMES.len()).map(image_id).collect();
        assert_eq!(ids.len(), FRAMES.len());
        assert!(ids.iter().all(|id| *id <= MAX_IMAGE_ID));
    }

    #[test]
    fn deleting_the_wand_frees_the_whole_image_id_range() {
        assert_eq!(
            delete_bytes(),
            "\u{1b}_Ga=d,d=R,q=2,x=45056,y=45067\u{1b}\\"
        );
    }

    #[test]
    fn the_frames_are_real_pngs() {
        for (index, png) in FRAMES.iter().enumerate() {
            assert!(
                png.starts_with(b"\x89PNG\r\n\x1a\n"),
                "frame {index} is not a png"
            );
        }
    }
}
