//! Codex rust-v0.162.1 terminal-derived styles. Keep the f32 truncation, contrast
//! search and CIE76 quantization in sync with that release's tui/src/{color.rs,
//! style.rs,style/contrast.rs,terminal_palette.rs}; these are source identities,
//! not Yinz's destination theme calculations. Decorative/analytics palettes are excluded.
use yinz_types::agent::theme::{
    AgentThemeSnapshot, Availability, Channel, Definition, Role, SourceColor, SNAPSHOT_VERSION,
};
use yinz_types::codex_verified_version as verified_version;

type Rgb = (u8, u8, u8);
const FG: &str = "codex.terminal.foreground";
const BG: &str = "codex.terminal.background";
const BLUE_100: Rgb = (164, 205, 251);
const BLUE_200: Rgb = (99, 168, 248);

pub(super) fn observe(snapshot: &mut AgentThemeSnapshot, code: u8, rgb: Rgb) -> bool {
    // Later releases reuse these derivations; the resolver records the version difference.
    if snapshot.version != SNAPSHOT_VERSION
        || snapshot.provider != "codex"
        || matches!(snapshot.availability, Availability::Unavailable(_))
    {
        return false;
    }
    let (token, channel, role) = match code {
        10 => (FG, Channel::Foreground, Role::Foreground),
        11 => (BG, Channel::Background, Role::Background),
        _ => return false,
    };
    if observed(snapshot, token).is_some() {
        return false;
    }
    // These ordinary definitions also preserve partial observations across handoffs,
    // without adding a second snapshot format or guessing from the destination theme.
    push(
        &mut snapshot.definitions,
        token,
        channel,
        color(rgb),
        None,
        role,
    );
    if let Some((fg, bg)) = observed(snapshot, FG).zip(observed(snapshot, BG)) {
        derive(snapshot, fg, bg);
    }
    snapshot.finish();
    true
}

fn observed(snapshot: &AgentThemeSnapshot, token: &str) -> Option<Rgb> {
    snapshot
        .definitions
        .iter()
        .find_map(|d| (d.token == token).then(|| as_rgb(d.color)).flatten())
}

fn push(
    definitions: &mut Vec<Definition>,
    token: &str,
    channel: Channel,
    color: SourceColor,
    pair: Option<(SourceColor, SourceColor)>,
    role: Role,
) {
    definitions.push(Definition {
        token: token.into(),
        channel,
        color,
        pair,
        target: Some(role.into()),
    });
}

fn derive(snapshot: &mut AgentThemeSnapshot, fg: Rgb, bg: Rgb) {
    // The provider exposes default colors only when BOTH startup replies succeeded.
    let light = is_light(bg);
    let top = if light { (0, 0, 0) } else { (255, 255, 255) };
    let composer = blend(top, bg, if light { 0.04 } else { 0.12 });
    let history = blend(top, bg, if light { 0.02 } else { 0.16 });
    let preferred_accent = |background| {
        if is_light(background) {
            (28, 100, 200)
        } else {
            BLUE_200
        }
    };
    // Theme accents can be emitted directly or contrast-corrected on a painted surface.
    // Only independently resolved semantic foregrounds enter this expansion.
    let themed: Vec<_> = snapshot
        .definitions
        .iter()
        .filter_map(|d| {
            if d.channel != Channel::Foreground || d.token.starts_with("codex.terminal.") {
                return None;
            }
            d.target
                .zip(as_rgb(d.color))
                .map(|(target, rgb)| (d.token.clone(), target.role, rgb))
        })
        .collect();
    for indexed in [false, true] {
        let defs = &mut snapshot.definitions;
        let surfaces = [
            ("codex.terminal.composer-plan", best(composer, indexed)),
            ("codex.terminal.history-prompt", best(history, indexed)),
        ];
        for (token, surface) in surfaces {
            push(
                defs,
                token,
                Channel::Background,
                surface,
                None,
                Role::Surface,
            );
        }
        // ANSI reduction can make these collide with foregrounds corrected for
        // selection fills. Preserve their meaning on the actual unselected surface;
        // the existing compiler still decides all equally specific collisions.
        for (token, source, role) in [
            (
                "codex.terminal.secondary",
                foreground(blend(fg, bg, 0.6), bg, indexed),
                Role::Muted,
            ),
            (
                "codex.terminal.table-separator",
                best(blend(fg, bg, 0.2), indexed),
                Role::Subtle,
            ),
            (
                "codex.terminal.warning",
                foreground(
                    if light {
                        (139, 98, 20)
                    } else {
                        (196, 167, 103)
                    },
                    bg,
                    indexed,
                ),
                Role::Warning,
            ),
        ] {
            for background in [SourceColor::Default, surfaces[0].1, surfaces[1].1] {
                push(
                    defs,
                    token,
                    Channel::Foreground,
                    source,
                    Some((source, background)),
                    role,
                );
            }
        }
        push(
            defs,
            "codex.terminal.accent",
            Channel::Foreground,
            foreground(preferred_accent(bg), bg, indexed),
            None,
            Role::Accent,
        );

        let (preferred, alternate) = if light {
            (BLUE_100, BLUE_200)
        } else {
            (BLUE_200, BLUE_100)
        };
        let mut fill = best(preferred, indexed);
        let mut fill_rgb = as_rgb(fill).unwrap_or(preferred);
        if ratio(fill_rgb, bg) < 1.25 {
            let alternate_fill = best(alternate, indexed);
            let alternate_rgb = as_rgb(alternate_fill).unwrap_or(alternate);
            if ratio(alternate_rgb, bg) > ratio(fill_rgb, bg) {
                fill = alternate_fill;
                fill_rgb = alternate_rgb;
            }
        }
        let selection_fg = foreground((0, 0, 46), fill_rgb, indexed);
        let pair = Some((selection_fg, fill));
        push(
            defs,
            "codex.terminal.selection.foreground",
            Channel::Foreground,
            selection_fg,
            pair,
            Role::SelectionForeground,
        );
        push(
            defs,
            "codex.terminal.selection.background",
            Channel::Background,
            fill,
            pair,
            Role::SelectionBackground,
        );

        // The resume picker corrects the already-resolved secondary foreground
        // again against the painted selection, including ANSI quantization at
        // both steps. Keep this pair distinct from unselected secondary text.
        let secondary = foreground(blend(fg, bg, 0.6), bg, indexed);
        if let Some(secondary_rgb) = as_rgb(secondary) {
            let selected_secondary = foreground(secondary_rgb, fill_rgb, indexed);
            let pair = Some((selected_secondary, fill));
            for (token, channel, source, role) in [
                (
                    "codex.terminal.selection.secondary",
                    Channel::Foreground,
                    selected_secondary,
                    Role::SelectionForeground,
                ),
                (
                    "codex.terminal.selection.background",
                    Channel::Background,
                    fill,
                    Role::SelectionBackground,
                ),
            ] {
                push(defs, token, channel, source, pair, role);
            }
        }

        for background in [SourceColor::Default, surfaces[0].1, surfaces[1].1, fill] {
            let background_rgb = as_rgb(background).unwrap_or(bg);
            let accent = foreground(preferred_accent(background_rgb), background_rgb, indexed);
            push(
                defs,
                "codex.terminal.surface-accent",
                Channel::Foreground,
                accent,
                Some((accent, background)),
                Role::Accent,
            );
            let base = foreground(fg, background_rgb, indexed);
            push(
                defs,
                "codex.terminal.readable-foreground",
                Channel::Foreground,
                base,
                Some((base, background)),
                if background == fill {
                    Role::SelectionForeground
                } else {
                    Role::Foreground
                },
            );
            for (token, role, preferred) in &themed {
                let resolved = foreground(*preferred, background_rgb, indexed);
                push(
                    defs,
                    token,
                    Channel::Foreground,
                    resolved,
                    Some((resolved, background)),
                    *role,
                );
            }
        }
        for (token, role, preferred) in &themed {
            push(
                defs,
                token,
                Channel::Foreground,
                best(*preferred, indexed),
                None,
                *role,
            );
        }
    }
    snapshot.provenance.push(
        concat!(
            "codex-rust-v",
            verified_version!(),
            "/observed-terminal-colors"
        )
        .into(),
    );
}

fn color((r, g, b): Rgb) -> SourceColor {
    SourceColor::Rgb(r, g, b)
}
fn as_rgb(color: SourceColor) -> Option<Rgb> {
    match color {
        SourceColor::Rgb(r, g, b) => Some((r, g, b)),
        SourceColor::Index(i) if i >= 16 => Some(yinz_types::colors::eightbit_to_rgb(i)),
        _ => None,
    }
}

fn best(preferred: Rgb, indexed: bool) -> SourceColor {
    if indexed {
        (16..=255u8)
            .min_by(|a, b| {
                perceptual_distance(yinz_types::colors::eightbit_to_rgb(*a), preferred).total_cmp(
                    &perceptual_distance(yinz_types::colors::eightbit_to_rgb(*b), preferred),
                )
            })
            .map_or(SourceColor::Default, SourceColor::Index)
    } else {
        color(preferred)
    }
}

fn foreground(preferred: Rgb, background: Rgb, indexed: bool) -> SourceColor {
    if indexed {
        (16..=255u8)
            .filter(|i| ratio(yinz_types::colors::eightbit_to_rgb(*i), background) >= 4.5)
            .min_by(|a, b| {
                perceptual_distance(yinz_types::colors::eightbit_to_rgb(*a), preferred).total_cmp(
                    &perceptual_distance(yinz_types::colors::eightbit_to_rgb(*b), preferred),
                )
            })
            .map_or(SourceColor::Default, SourceColor::Index)
    } else if ratio(preferred, background) >= 4.5 {
        color(preferred)
    } else {
        let endpoint = if ratio((0, 0, 0), background) >= ratio((255, 255, 255), background) {
            (0, 0, 0)
        } else {
            (255, 255, 255)
        };
        color(
            (1..=255)
                .map(|step| blend(endpoint, preferred, step as f32 / 255.0))
                .find(|candidate| ratio(*candidate, background) >= 4.5)
                .unwrap_or(endpoint),
        )
    }
}

fn ratio(a: Rgb, b: Rgb) -> f64 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
fn luminance(rgb: Rgb) -> f64 {
    let [r, g, b] = [rgb.0, rgb.1, rgb.2].map(|channel| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Accept exactly a reply to the requested channel, using the provider's component
/// scaling (including 1/2/3/4-digit components and rgba). Invalid replies teach nothing.
pub(crate) fn parse_reply(code: u8, reply: &[u8]) -> Option<Rgb> {
    let reply = std::str::from_utf8(reply).ok()?;
    let prefix = format!("\x1b]{code};");
    let body = reply.strip_prefix(&prefix)?;
    let body = body
        .strip_suffix('\x07')
        .or_else(|| body.strip_suffix("\x1b\\"))?;
    let (prefix, values) = body.trim().split_once(':')?;
    let rgba = prefix.eq_ignore_ascii_case("rgba");
    if !rgba && !prefix.eq_ignore_ascii_case("rgb") {
        return None;
    }
    let component = |s: &str| {
        if !(1..=4).contains(&s.len()) || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let value = u32::from(u16::from_str_radix(s, 16).ok()?);
        Some((value * 255 / ((1u32 << (s.len() * 4)) - 1)) as u8)
    };
    let mut parts = values.split('/');
    let rgb = (
        component(parts.next()?)?,
        component(parts.next()?)?,
        component(parts.next()?)?,
    );
    if rgba {
        component(parts.next()?)?;
    }
    parts.next().is_none().then_some(rgb)
}

#[cfg(test)]
#[path = "unit/codex_0_162_1.rs"]
mod tests;

fn is_light(bg: (u8, u8, u8)) -> bool {
    let (r, g, b) = bg;
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    y > 128.0
}

fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let r = (fg.0 as f32 * alpha + bg.0 as f32 * (1.0 - alpha)) as u8;
    let g = (fg.1 as f32 * alpha + bg.1 as f32 * (1.0 - alpha)) as u8;
    let b = (fg.2 as f32 * alpha + bg.2 as f32 * (1.0 - alpha)) as u8;
    (r, g, b)
}

/// Returns the perceptual color distance between two RGB colors.
/// Uses the CIE76 formula (Euclidean distance in Lab space approximation).
fn perceptual_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    // Convert sRGB to linear RGB
    fn srgb_to_linear(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    // Convert RGB to XYZ
    fn rgb_to_xyz(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
        let r = srgb_to_linear(r);
        let g = srgb_to_linear(g);
        let b = srgb_to_linear(b);

        let x = r * 0.4124 + g * 0.3576 + b * 0.1805;
        let y = r * 0.2126 + g * 0.7152 + b * 0.0722;
        let z = r * 0.0193 + g * 0.1192 + b * 0.9505;
        (x, y, z)
    }

    // Convert XYZ to Lab
    fn xyz_to_lab(x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        // D65 reference white
        let xr = x / 0.95047;
        let yr = y / 1.00000;
        let zr = z / 1.08883;

        fn f(t: f32) -> f32 {
            if t > 0.008856 {
                t.powf(1.0 / 3.0)
            } else {
                7.787 * t + 16.0 / 116.0
            }
        }

        let fx = f(xr);
        let fy = f(yr);
        let fz = f(zr);

        let l = 116.0 * fy - 16.0;
        let a = 500.0 * (fx - fy);
        let b = 200.0 * (fy - fz);
        (l, a, b)
    }

    let (x1, y1, z1) = rgb_to_xyz(a.0, a.1, a.2);
    let (x2, y2, z2) = rgb_to_xyz(b.0, b.1, b.2);

    let (l1, a1, b1) = xyz_to_lab(x1, y1, z1);
    let (l2, a2, b2) = xyz_to_lab(x2, y2, z2);

    let dl = l1 - l2;
    let da = a1 - a2;
    let db = b1 - b2;

    (dl * dl + da * da + db * db).sqrt()
}
