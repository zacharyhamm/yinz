use super::*;
use crate::splash::geometry::{inside, Sample};

#[test]
fn braille_bits_and_colors_only_include_occupied_dots() {
    for (i, bit) in BITS.into_iter().enumerate() {
        let mut dots = [Dot::default(); 8];
        dots[i] = Dot {
            depth: 1.0,
            color: [100.0, 120.0, 140.0],
        };
        assert_eq!(pack(&dots), (bit, [100, 120, 140]));
    }
    let mut dots = [Dot::default(); 8];
    for (i, color) in [
        [100.0, 120.0, 140.0],
        [200.0, 180.0, 160.0],
        [0.0, 60.0, 100.0],
    ]
    .into_iter()
    .enumerate()
    {
        dots[i] = Dot { depth: 0.0, color };
    }
    assert_eq!(pack(&dots), (11, [100, 120, 133]));
}

#[test]
fn contours_preserve_holes_and_the_i_dot_is_part_of_one_glyph() {
    let square_with_hole = [
        [-1.0, -1.0, 1.0, -1.0],
        [1.0, -1.0, 1.0, 1.0],
        [1.0, 1.0, -1.0, 1.0],
        [-1.0, 1.0, -1.0, -1.0],
        [-0.5, -0.5, -0.5, 0.5],
        [-0.5, 0.5, 0.5, 0.5],
        [0.5, 0.5, 0.5, -0.5],
        [0.5, -0.5, -0.5, -0.5],
    ];
    assert!(!inside(&square_with_hole, 0.0, 0.0));
    assert!(inside(&square_with_hole, 0.75, 0.0));
    assert!(!inside(&square_with_hole, 2.0, 0.0));
    assert_eq!(OUTLINES.len(), 4);
    // The i has occupied samples in both its detached dot and its stem.
    let i = &GLYPHS[1];
    assert!(i.samples.iter().any(|s| s.position[1] < -0.7));
    assert!(i.samples.iter().any(|s| s.position[1] > 0.0));
}

#[test]
fn turns_are_in_order_and_shimmer_follows_the_fourth_turn() {
    for letter in 0..4 {
        let at = Duration::from_millis(letter as u64 * 500 + 250);
        for other in 0..4 {
            if other == letter {
                assert!((angle(at, other) - std::f32::consts::PI).abs() < 0.001);
            } else {
                assert_eq!(angle(at, other), 0.0);
            }
        }
        assert_eq!(
            angle(Duration::from_millis(letter as u64 * 500), letter),
            0.0
        );
        assert_eq!(
            angle(Duration::from_millis(letter as u64 * 500 + 500), letter),
            0.0
        );
        assert_eq!(shimmer(at, 0.0, 4.0), 0.0);
    }
    assert!(shimmer(Duration::from_millis(2200), -1.3, 4.0) > 0.99);
    assert!(shimmer(Duration::from_millis(2600), 1.3, 4.0) > 0.99);
    assert_eq!(shimmer(Duration::from_millis(2800), 0.0, 4.0), 0.0);
}

#[test]
fn nearer_samples_win_independent_of_draw_order() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let mut renderer = Renderer::new(stage, style(false));
    let front = Sample {
        position: [0.0, 0.0, 0.1],
        normal: [0.0, 0.0, 1.0],
    };
    let back = Sample {
        position: [0.0, 0.0, -0.1],
        normal: [0.0, 0.0, -1.0],
    };
    renderer.project(&front, 0.0, 0.0, 1.0, 20.0, Duration::ZERO, 4.0);
    renderer.project(&back, 0.0, 0.0, 1.0, 20.0, Duration::ZERO, 4.0);
    let center = stage.rows * 2 * stage.cols * 2 + stage.cols;
    assert_eq!(renderer.dots[center].depth, 0.1);
}

fn style(light: bool) -> StyleDeclaration {
    StyleDeclaration {
        background: PaletteColor::Rgb(if light { (245, 245, 245) } else { (20, 22, 26) }),
        base: PaletteColor::Rgb(if light { (35, 40, 50) } else { (210, 221, 235) }),
        ..Default::default()
    }
}

#[test]
fn shimmer_increases_contrast_on_both_light_and_dark_backgrounds() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    for light in [false, true] {
        let mut renderer = Renderer::new(stage, style(light));
        renderer.frame(Duration::ZERO);
        let resting = renderer.dots.clone();
        renderer.frame(Duration::from_millis(2400));
        let mut highlighted = 0;
        for (before, after) in resting.iter().zip(&renderer.dots) {
            if before.depth.is_finite() && before.color != after.color {
                for channel in 0..3 {
                    if light {
                        assert!(after.color[channel] < before.color[channel]);
                    } else {
                        assert!(after.color[channel] > before.color[channel]);
                    }
                }
                highlighted += 1;
            }
        }
        assert!(highlighted > 0);
    }
}

fn emitted_contrast(foreground: [u8; 3], background: [u8; 3]) -> f64 {
    let relative_luminance = |color: [u8; 3]| {
        color
            .into_iter()
            .zip([0.2126, 0.7152, 0.0722])
            .map(|(channel, weight)| {
                let channel = f64::from(channel) / 255.0;
                weight
                    * if channel <= 0.04045 {
                        channel / 12.92
                    } else {
                        ((channel + 0.055) / 1.055).powf(2.4)
                    }
            })
            .sum::<f64>()
    };
    let a = relative_luminance(foreground);
    let b = relative_luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn relative_luminance_linearizes_srgb_and_highlights_use_the_stronger_endpoint() {
    for (color, expected) in [
        ([0.0; 3], 0.0),
        ([255.0; 3], 1.0),
        ([255.0, 0.0, 0.0], 0.2126),
        ([0.0, 255.0, 0.0], 0.7152),
        ([0.0, 0.0, 255.0], 0.0722),
        ([10.0; 3], 0.00303527),
        ([11.0; 3], 0.00334654),
        ([128.0; 3], 0.2158605),
    ] {
        assert!((luminance(color) - expected).abs() < 0.0000001);
    }
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    for (background, expected) in [
        ((117, 117, 117), [255.0; 3]),
        ((118, 118, 118), [0.0; 3]),
        ((0, 150, 0), [0.0; 3]),
    ] {
        let renderer = Renderer::new(
            stage,
            StyleDeclaration {
                background: PaletteColor::Rgb(background),
                ..Default::default()
            },
        );
        assert_eq!(renderer.highlight, expected);
    }
}

#[test]
fn contrast_correction_preserves_readable_colors_and_minimally_blends_other_colors() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    for gray in 0..=255 {
        let background = [gray; 3];
        let renderer = Renderer::new(
            stage,
            StyleDeclaration {
                background: PaletteColor::Rgb((gray, gray, gray)),
                ..Default::default()
            },
        );
        for color in [
            background,
            [0; 3],
            [255; 3],
            [10; 3],
            [128; 3],
            [200, 50, 120],
            [30, 220, 80],
            [255, 200, 0],
        ] {
            let corrected =
                ensure_contrast(color, renderer.background_luminance, renderer.highlight);
            assert!(emitted_contrast(corrected, background) >= 4.5);
            if emitted_contrast(color, background) >= 4.5 {
                assert_eq!(corrected, color);
            } else {
                for channel in 0..3 {
                    if renderer.highlight == [0.0; 3] {
                        assert!(corrected[channel] <= color[channel]);
                    } else {
                        assert!(corrected[channel] >= color[channel]);
                    }
                }
            }
        }
        // For gray on gray, the nearest qualifying byte is the exact minimal
        // blend. This also exercises rounding near the contrast threshold.
        let expected = (0..=255)
            .filter(|&value| emitted_contrast([value; 3], background) >= 4.5)
            .filter(|&value| {
                if renderer.highlight == [0.0; 3] {
                    value < gray
                } else {
                    value > gray
                }
            })
            .min_by_key(|value| value.abs_diff(gray))
            .unwrap();
        assert_eq!(
            ensure_contrast(
                background,
                renderer.background_luminance,
                renderer.highlight
            ),
            [expected; 3]
        );
    }
}

#[test]
fn visible_cells_keep_their_contrast_floor_and_exact_background_throughout_the_animation() {
    let foreground = regex::Regex::new("\x1b\\[38;2;([0-9]+);([0-9]+);([0-9]+)m([⠁-⣿])").unwrap();
    let background = regex::Regex::new("\x1b\\[48;2;([0-9]+);([0-9]+);([0-9]+)m").unwrap();
    let rgb_style = |base, background| StyleDeclaration {
        base: PaletteColor::Rgb(base),
        background: PaletteColor::Rgb(background),
        ..Default::default()
    };
    for style in [
        style(false),
        style(true),
        rgb_style((0, 0, 0), (117, 117, 117)),
        rgb_style((255, 255, 255), (118, 118, 118)),
        rgb_style((138, 130, 140), (128, 120, 135)),
        rgb_style((20, 22, 26), (20, 22, 26)),
        rgb_style((245, 245, 245), (245, 245, 245)),
        rgb_style((0, 160, 0), (0, 150, 0)),
        StyleDeclaration {
            base: PaletteColor::EightBit(255),
            background: PaletteColor::EightBit(16),
            ..Default::default()
        },
        StyleDeclaration {
            base: PaletteColor::EightBit(232),
            background: PaletteColor::EightBit(255),
            ..Default::default()
        },
        StyleDeclaration {
            base: PaletteColor::EightBit(244),
            background: PaletteColor::EightBit(244),
            ..Default::default()
        },
    ] {
        let expected_background = rgb(style.background).map(|channel| channel as u8);
        for size in [Size { rows: 24, cols: 80 }, Size { rows: 10, cols: 28 }] {
            let mut renderer = Renderer::new(Stage::new(size).unwrap(), style);
            // Every scheduled frame, plus rotation and edge-on poses between
            // frame boundaries. Both Braille resolutions must stay readable.
            for millis in (0..=3000)
                .step_by(50)
                .chain([120, 180, 320, 680, 820, 1175, 1180, 1320, 1675, 1680, 1820])
            {
                let frame = renderer.frame(Duration::from_millis(millis));
                let parse_color = |capture: regex::Captures<'_>| {
                    std::array::from_fn(|i| capture[i + 1].parse::<u8>().unwrap())
                };
                let backgrounds: Vec<_> =
                    background.captures_iter(&frame).map(parse_color).collect();
                assert_eq!(backgrounds, [expected_background]);
                let [r, g, b] = expected_background;
                assert!(frame.starts_with(&format!("\x1b[m\x1b[48;2;{r};{g};{b}m\x1b[2J\x1b[H")));
                let mut visible = 0;
                for cell in foreground.captures_iter(&frame) {
                    let color = parse_color(cell);
                    let ratio = emitted_contrast(color, expected_background);
                    assert!(
                        ratio >= 4.5,
                        "{style:?}, {size:?}, {millis}ms: {color:?} has contrast {ratio}"
                    );
                    visible += 1;
                }
                assert!(visible > 0);
                assert_eq!(
                    visible,
                    frame.chars().filter(|c| ('⠁'..='⣿').contains(c)).count()
                );
            }
        }
    }
}

#[test]
fn representative_frames_have_stable_silhouettes_and_theme_contrast() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*[a-zA-Z]").unwrap();
    let cursor = regex::Regex::new("\x1b\\[[0-9]+;[0-9]+H").unwrap();
    let mut dark = Renderer::new(stage, style(false));
    let mut light = Renderer::new(stage, style(true));
    let mut frames = String::new();
    for (name, millis) in [
        ("rest", 0),
        ("y turning", 120),
        ("i edge-on", 680),
        ("n turning", 1175),
        ("z turning", 1675),
        ("shimmer", 2400),
    ] {
        let elapsed = Duration::from_millis(millis);
        let dark_frame = dark.frame(elapsed);
        let light_frame = light.frame(elapsed);
        assert_ne!(dark_frame, light_frame);
        assert_eq!(
            ansi.replace_all(&dark_frame, ""),
            ansi.replace_all(&light_frame, "")
        );
        frames.push_str(&format!("{name}\n"));
        // Each row starts with a cursor-position sequence after background setup.
        let rows = cursor.split(&dark_frame).skip(1);
        for row in rows {
            let text = ansi.replace_all(row, "");
            frames.push_str(text.trim_end());
            frames.push('\n');
        }
    }
    insta::assert_snapshot!(frames);
    assert_eq!(
        dark.frame(Duration::ZERO),
        dark.frame(Duration::from_secs(3))
    );
}
