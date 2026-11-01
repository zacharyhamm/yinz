//! Cached beveled surfaces sampled from the generated SVG contours.

use super::outlines::OUTLINES;
use std::sync::LazyLock;

const GRID: usize = 160;
const EXTENT: f32 = 1.1;
const STEP: f32 = 2.0 * EXTENT / (GRID - 1) as f32;

pub(super) struct Outline {
    pub center: f32,
    pub segments: &'static [[f32; 4]],
}

pub(super) struct Sample {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

pub(super) struct Glyph {
    pub center: f32,
    pub samples: Vec<Sample>,
}

pub(super) static GLYPHS: LazyLock<[Glyph; 4]> = LazyLock::new(|| {
    OUTLINES.each_ref().map(|outline| Glyph {
        center: outline.center,
        samples: surface(&distance_field(outline)),
    })
});

pub(super) fn inside(segments: &[[f32; 4]], x: f32, y: f32) -> bool {
    let mut winding = 0;
    for &[ax, ay, bx, by] in segments {
        if ((ay <= y && y < by) || (by <= y && y < ay))
            && ax + (y - ay) * (bx - ax) / (by - ay) <= x
        {
            winding += if by > ay { 1 } else { -1 };
        }
    }
    winding != 0
}

fn distance_field(outline: &Outline) -> Vec<f32> {
    let mask: Vec<_> = (0..GRID * GRID)
        .map(|i| {
            inside(
                outline.segments,
                (i % GRID) as f32 * STEP - EXTENT,
                (i / GRID) as f32 * STEP - EXTENT,
            )
        })
        .collect();
    let mut distance = vec![GRID as f32; GRID * GRID];
    for y in 1..GRID - 1 {
        for x in 1..GRID - 1 {
            let i = y * GRID + x;
            if [i - 1, i + 1, i - GRID, i + GRID]
                .iter()
                .any(|&j| mask[i] != mask[j])
            {
                distance[i] = 0.5;
            }
        }
    }
    for reverse in [false, true] {
        for row in 1..GRID - 1 {
            for col in 1..GRID - 1 {
                let (x, y) = if reverse {
                    (GRID - 1 - col, GRID - 1 - row)
                } else {
                    (col, row)
                };
                let i = y * GRID + x;
                let neighbors = if reverse {
                    [i + 1, i + GRID, i + GRID - 1, i + GRID + 1]
                } else {
                    [i - 1, i - GRID, i - GRID - 1, i - GRID + 1]
                };
                for (n, neighbor) in neighbors.into_iter().enumerate() {
                    distance[i] = distance[i].min(
                        distance[neighbor] + if n < 2 { 1.0 } else { std::f32::consts::SQRT_2 },
                    );
                }
            }
        }
    }
    distance
        .iter()
        .zip(mask)
        .map(|(d, inside)| d * STEP * if inside { 1.0 } else { -1.0 })
        .collect()
}

fn surface(field: &[f32]) -> Vec<Sample> {
    let mut samples = Vec::new();
    let half_depth = 0.09;
    let bevel = 0.04;
    for row in 1..GRID - 1 {
        for col in 1..GRID - 1 {
            let i = row * GRID + col;
            let d = field[i];
            let x = col as f32 * STEP - EXTENT;
            let y = row as f32 * STEP - EXTENT;
            let dx = field[i + 1] - field[i - 1];
            let dy = field[i + GRID] - field[i - GRID];
            let length = dx.hypot(dy).max(f32::EPSILON);
            let (gx, gy) = (dx / length, dy / length);
            if d > 0.0 {
                let edge = (1.0 - d / bevel).clamp(0.0, 1.0);
                let nz = (1.0 - edge * edge).sqrt();
                for sign in [-1.0, 1.0] {
                    samples.push(Sample {
                        position: [x, y, sign * (half_depth - bevel + bevel * nz)],
                        normal: [-gx * edge, -gy * edge, sign * nz],
                    });
                }
            }
            if d.abs() < STEP * 0.8 {
                // Include walls so the letter remains solid at an edge-on pose.
                for z in -4..=4 {
                    samples.push(Sample {
                        position: [
                            x - gx * d,
                            y - gy * d,
                            z as f32 * (half_depth - bevel) / 4.0,
                        ],
                        normal: [-gx, -gy, 0.0],
                    });
                }
            }
        }
    }
    samples
}
