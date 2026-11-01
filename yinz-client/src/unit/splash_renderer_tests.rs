use super::super::geometry::{inside, Sample};
use super::*;

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
fn one_eased_turn_finishes_before_the_shimmer() {
    assert_eq!(angle(Duration::ZERO), 0.0);
    assert!(angle(Duration::from_millis(175)) < std::f32::consts::FRAC_PI_2);
    assert!((angle(Duration::from_millis(350)) - std::f32::consts::PI).abs() < 0.001);
    assert!(angle(Duration::from_millis(525)) > 3.0 * std::f32::consts::FRAC_PI_2);
    for millis in [700, 875, 1050, 1225, 1400, 2000] {
        assert_eq!(angle(Duration::from_millis(millis)), 0.0);
    }
    for millis in [0, 175, 350, 525, 699, 1400, 2000] {
        assert_eq!(shimmer(Duration::from_millis(millis), 0.0, 4.0), 0.0);
    }
    assert!(shimmer(Duration::from_millis(875), -1.3, 4.0) > 0.99);
    assert!(shimmer(Duration::from_millis(1050), 0.0, 4.0) > 0.99);
    assert!(shimmer(Duration::from_millis(1225), 1.3, 4.0) > 0.99);
}

#[test]
fn nearer_samples_win_independent_of_draw_order() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let mut renderer = Renderer::new(stage, style(false), accent(false));
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

fn accent(light: bool) -> PaletteColor {
    PaletteColor::Rgb(if light { (86, 115, 0) } else { (80, 250, 123) })
}

#[test]
fn shimmer_blends_toward_the_accent_while_rotation_keeps_neutral_lighting() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    for light in [false, true] {
        let mut reference = Renderer::new(stage, style(light), accent(light));
        for accent in [accent(light), PaletteColor::EightBit(201)] {
            let mut renderer = Renderer::new(stage, style(light), accent);
            for millis in [0, 175, 252, 350, 525, 700, 1400] {
                let elapsed = Duration::from_millis(millis);
                assert_eq!(renderer.frame(elapsed), reference.frame(elapsed));
            }
            renderer.frame(Duration::ZERO);
            let resting = renderer.dots.clone();
            renderer.frame(Duration::from_millis(1050));
            let mut highlighted = 0;
            let accent = rgb(accent);
            for (before, after) in resting.iter().zip(&renderer.dots) {
                assert_eq!(before.depth, after.depth);
                if before.depth.is_finite() && before.color != after.color {
                    for ((before, after), accent) in
                        before.color.into_iter().zip(after.color).zip(accent)
                    {
                        assert!((after - accent).abs() <= (before - accent).abs());
                        assert!(after >= before.min(accent));
                        assert!(after <= before.max(accent));
                    }
                    highlighted += 1;
                }
            }
            assert!(highlighted > 0);
        }
    }
}

#[test]
fn indexed_and_rgb_accents_emit_the_same_shimmer() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let mut indexed = Renderer::new(stage, style(false), PaletteColor::EightBit(201));
    let mut truecolor = Renderer::new(stage, style(false), PaletteColor::Rgb(eightbit_to_rgb(201)));
    assert_eq!(
        indexed.frame(Duration::from_millis(1050)),
        truecolor.frame(Duration::from_millis(1050))
    );
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
            accent(false),
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
            accent(false),
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
    let foreground = regex::Regex::new("\x1b\\[38;2;([0-9]+);([0-9]+);([0-9]+)m").unwrap();
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
            for accent in [
                accent(false),
                accent(true),
                style.background,
                PaletteColor::EightBit(201),
            ] {
                let mut renderer = Renderer::new(Stage::new(size).unwrap(), style, accent);
                // Every scheduled frame, plus rotation and edge-on poses between
                // frame boundaries. Both Braille resolutions must stay readable.
                for millis in (0..=1400)
                    .step_by(50)
                    .chain([168, 252, 448, 650, 699, 700, 701, 875, 1050, 1225, 1399])
                {
                    let frame = renderer.frame(Duration::from_millis(millis));
                    let parse_color = |capture: regex::Captures<'_>| {
                        std::array::from_fn(|i| capture[i + 1].parse::<u8>().unwrap())
                    };
                    let backgrounds: Vec<_> =
                        background.captures_iter(&frame).map(parse_color).collect();
                    assert_eq!(backgrounds, [expected_background]);
                    let [r, g, b] = expected_background;
                    assert!(
                        frame.starts_with(&format!("\x1b[m\x1b[48;2;{r};{g};{b}m\x1b[2J\x1b[H"))
                    );
                    let mut visible = 0;
                    for cell in foreground.captures_iter(&frame) {
                        let color = parse_color(cell);
                        let ratio = emitted_contrast(color, expected_background);
                        assert!(
                            ratio >= 4.5,
                            "{style:?}, {accent:?}, {size:?}, {millis}ms: \
                             {color:?} has contrast {ratio}"
                        );
                        visible += 1;
                    }
                    assert!(visible > 0);
                    assert_eq!(
                        visible,
                        frame.chars().filter(|c| ('⠁'..='⣿').contains(c)).count()
                            + usize::from(!renderer.quote.is_empty())
                    );
                }
            }
        }
    }
}

#[test]
fn representative_frames_have_stable_silhouettes_and_theme_contrast() {
    let stage = Stage::new(Size { rows: 24, cols: 80 }).unwrap();
    let ansi = regex::Regex::new("\x1b\\[[0-9;]*[a-zA-Z]").unwrap();
    let cursor = regex::Regex::new("\x1b\\[[0-9]+;[0-9]+H").unwrap();
    let mut dark = Renderer::new(stage, style(false), accent(false));
    let mut light = Renderer::new(stage, style(true), accent(true));
    let mut frames = String::new();
    for (name, millis) in [
        ("rest", 0),
        ("all turning", 175),
        ("all edge-on", 252),
        ("all half-turned", 350),
        ("all returning", 525),
        ("shimmer", 1050),
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
        // Preserve cursor offsets so the snapshot shows the quote's centering
        // and the blank row separating it from the logo.
        let positions: Vec<_> = cursor.find_iter(&dark_frame).collect();
        let mut previous_row = stage.top - 1;
        for (index, position) in positions.iter().enumerate() {
            let (row, col) = position.as_str()[2..position.len() - 1]
                .split_once(';')
                .unwrap();
            let row = row.parse::<usize>().unwrap();
            let col = col.parse::<usize>().unwrap();
            for _ in previous_row + 1..row {
                frames.push('\n');
            }
            let end = positions
                .get(index + 1)
                .map_or(dark_frame.len(), |next| next.start());
            let text = ansi.replace_all(&dark_frame[position.end()..end], "");
            frames.push_str(&" ".repeat(col - stage.left));
            frames.push_str(text.trim_end());
            frames.push('\n');
            previous_row = row;
        }
    }
    insta::assert_snapshot!(frames);
    assert_eq!(
        dark.frame(Duration::ZERO),
        dark.frame(Duration::from_millis(700))
    );
    assert_eq!(
        dark.frame(Duration::ZERO),
        dark.frame(Duration::from_millis(1400))
    );
}

#[test]
fn quote_wraps_to_the_logo_and_remains_centered_after_resizing() {
    let mut renderer = Renderer::new(
        Stage::new(Size { rows: 24, cols: 80 }).unwrap(),
        style(false),
        accent(false),
    );
    let positioned_text = regex::Regex::new("\x1b\\[([0-9]+);([0-9]+)H([^\x1b]*)").unwrap();
    for size in [
        Size { rows: 24, cols: 80 },
        Size { rows: 36, cols: 41 },
        Size { rows: 36, cols: 28 },
        Size { rows: 10, cols: 28 },
        Size {
            rows: 40,
            cols: 120,
        },
    ] {
        renderer.resize(Stage::new(size).unwrap());
        let frame = renderer.frame(Duration::ZERO);
        let lines: Vec<_> = positioned_text
            .captures_iter(&frame)
            .filter(|capture| {
                capture[1].parse::<usize>().unwrap() > renderer.stage.top + renderer.stage.rows
            })
            .collect();
        if size.rows == 10 {
            assert!(lines.is_empty());
            continue;
        }
        let logo_width = (size.cols - 2).min(52);
        let left = (size.cols - logo_width) / 2 + 1;
        assert!(lines.len() > 1);
        for line in &lines {
            let width = line[3].width();
            assert!(width <= logo_width);
            assert_eq!(
                line[2].parse::<usize>().unwrap(),
                left + (logo_width - width) / 2
            );
            assert!(line[1].parse::<usize>().unwrap() < size.rows);
        }
        let attribution = lines
            .iter()
            .position(|line| line[3].starts_with("- "))
            .unwrap();
        assert_eq!(
            lines[attribution..]
                .iter()
                .map(|line| &line[3])
                .collect::<Vec<_>>()
                .join(" "),
            ATTRIBUTION
        );
        assert_eq!(
            lines[..attribution]
                .iter()
                .map(|line| &line[3])
                .collect::<Vec<_>>()
                .join(" "),
            QUOTE
        );
    }
}
