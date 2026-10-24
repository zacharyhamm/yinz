//! Compiled source-style lookups. Neither cells nor raw terminal captures are recolored.
use crate::panes::terminal_character::{AnsiCode, CharacterStyles};
use std::collections::BTreeMap;
use yinz_types::agent::theme::{AgentThemeSnapshot, Channel, MatchKey, Role, SourceColor, Target};
use yinz_types::{colors::PaletteColor, style::Style};

mod codex_0_162_1;
pub(crate) use codex_0_162_1::parse_reply as terminal_color_reply;

#[derive(Debug)]
struct Resolved {
    color: AnsiCode,
    theme_background: bool,
    selection: bool,
}

#[derive(Debug)]
pub struct Mapping {
    keys: BTreeMap<MatchKey, Option<Resolved>>,
    selection_foreground: AnsiCode,
}

impl Mapping {
    pub fn new(snapshot: &AgentThemeSnapshot, style: Style) -> Self {
        Self {
            keys: snapshot
                .compile()
                .0
                .into_iter()
                .map(|(key, target)| {
                    (
                        key,
                        target.map(|target| Resolved {
                            color: AnsiCode::RgbCode(resolve(target, style)),
                            theme_background: target.role == Role::Background && !target.shimmer,
                            selection: target.role == Role::SelectionBackground,
                        }),
                    )
                })
                .collect(),
            selection_foreground: AnsiCode::RgbCode(resolve(
                Role::SelectionForeground.into(),
                style,
            )),
        }
    }

    fn lookup(&self, key: MatchKey) -> Option<&Resolved> {
        // An ambiguous pair intentionally blocks a less specific single-color match.
        self.keys
            .get(&key)
            .or_else(|| self.keys.get(&MatchKey { pair: None, ..key }))
            .and_then(Option::as_ref)
    }

    pub fn apply(&self, mut styles: CharacterStyles) -> CharacterStyles {
        let foreground = source(styles.foreground);
        let background = source(styles.background);
        let pair = foreground.zip(background);
        let mut selected = false;
        let mut paired_foreground = false;
        for (channel, color) in [
            (Channel::Foreground, &mut styles.foreground),
            (Channel::Background, &mut styles.background),
            (Channel::Underline, &mut styles.underline_color),
        ] {
            let Some(original) = source(*color) else {
                continue;
            };
            let key = MatchKey {
                channel,
                color: original,
                pair,
            };
            // Only a provider-defined pair has a key with `pair` set.
            paired_foreground |= channel == Channel::Foreground
                && pair.is_some()
                && matches!(self.keys.get(&key), Some(Some(_)));
            if let Some(mapped) = self.lookup(key) {
                *color = Some(mapped.color);
                selected |= channel == Channel::Background && mapped.selection;
            }
        }
        // Agents that paint their own selection (Claude) keep each cell's foreground, which
        // was chosen for the text background; the destination selection pair stays readable.
        // A foreground the provider paired with this background (Codex contrast-corrects
        // accents against its fill) keeps its own role.
        if selected && !paired_foreground {
            styles.foreground = Some(self.selection_foreground);
        }
        styles
    }

    fn theme_background(&self, styles: CharacterStyles) -> bool {
        let Some(color) = source(styles.background) else {
            return false;
        };
        self.lookup(MatchKey {
            channel: Channel::Background,
            color,
            pair: source(styles.foreground).zip(Some(color)),
        })
        .is_some_and(|resolved| resolved.theme_background)
    }
}

fn source(color: Option<AnsiCode>) -> Option<SourceColor> {
    match color {
        None | Some(AnsiCode::Reset) => Some(SourceColor::Default),
        Some(AnsiCode::ColorIndex(index)) => Some(SourceColor::Index(index)),
        Some(AnsiCode::NamedColor(named)) => Some(SourceColor::Index(named as u8)),
        Some(AnsiCode::RgbCode((r, g, b))) => Some(SourceColor::Rgb(r, g, b)),
        _ => None,
    }
}

fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), percent: u16) -> (u8, u8, u8) {
    let channel =
        |f: u8, b: u8| ((u16::from(f) * percent + u16::from(b) * (100 - percent) + 50) / 100) as u8;
    (
        channel(fg.0, bg.0),
        channel(fg.1, bg.1),
        channel(fg.2, bg.2),
    )
}

fn rgb(color: PaletteColor, style: Style) -> (u8, u8, u8) {
    match color {
        PaletteColor::Rgb(rgb) => rgb,
        PaletteColor::EightBit(index) => {
            if let Some(terminal) = style.terminal
                && let Some(rgb) = terminal.ansi.get(usize::from(index))
            {
                return *rgb;
            }
            yinz_types::colors::eightbit_to_rgb(index)
        },
    }
}

fn resolve(target: Target, style: Style) -> (u8, u8, u8) {
    let fg = style
        .terminal
        .map(|t| t.foreground)
        .unwrap_or_else(|| rgb(style.colors.text_unselected.base, style));
    let bg = style
        .terminal
        .map(|t| t.background)
        .unwrap_or_else(|| rgb(style.colors.text_unselected.background, style));
    let success = rgb(style.colors.exit_code_success.base, style);
    let error = rgb(style.colors.exit_code_error.base, style);
    let result = match target.role {
        Role::Foreground => fg,
        Role::Background => bg,
        Role::Muted => blend(fg, bg, 65),
        Role::Subtle => blend(fg, bg, 40),
        Role::Accent => rgb(style.colors.text_unselected.emphasis_1, style),
        Role::Success => success,
        Role::Error => error,
        Role::Warning => rgb(style.colors.exit_code_error.emphasis_0, style),
        Role::SelectionForeground => rgb(style.colors.text_selected.base, style),
        Role::SelectionBackground => rgb(style.colors.text_selected.background, style),
        Role::Surface => blend(fg, bg, 8),
        Role::AdditionBackground => blend(success, bg, 18),
        Role::RemovalBackground => blend(error, bg, 18),
        Role::AdditionDimmed => blend(success, bg, 10),
        Role::RemovalDimmed => blend(error, bg, 10),
        Role::AdditionWord => blend(success, bg, 30),
        Role::RemovalWord => blend(error, bg, 30),
    };
    if target.shimmer {
        blend(fg, result, 25)
    } else {
        result
    }
}

/// Shared with retained rows and completed frames. Theme changes recompile once per source
/// palette; a new managed run gets a different palette without relabeling old scrollback.
#[derive(Debug)]
pub struct SourcePalette {
    snapshot: std::cell::RefCell<Option<AgentThemeSnapshot>>,
    mapping: std::cell::RefCell<Option<Mapping>>,
}
impl SourcePalette {
    pub fn new(snapshot: Option<AgentThemeSnapshot>, style: Style) -> Self {
        let palette = Self {
            snapshot: std::cell::RefCell::new(snapshot),
            mapping: Default::default(),
        };
        palette.recompile(style);
        palette
    }
    pub fn snapshot(&self) -> Option<std::cell::Ref<'_, AgentThemeSnapshot>> {
        std::cell::Ref::filter_map(self.snapshot.borrow(), Option::as_ref).ok()
    }
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.borrow().is_some()
    }
    /// Enrich the same palette held by existing rows and completed frames. The observed
    /// source colors are immutable for this launch, even during theme previews.
    pub fn observe_terminal_color(&self, code: u8, rgb: (u8, u8, u8), style: Style) -> bool {
        let changed = self
            .snapshot
            .borrow_mut()
            .as_mut()
            .is_some_and(|snapshot| codex_0_162_1::observe(snapshot, code, rgb));
        if changed {
            self.recompile(style);
        }
        changed
    }
    pub fn recompile(&self, style: Style) {
        *self.mapping.borrow_mut() =
            if style.agent_theme_mapping == yinz_types::agent::theme::AgentThemeMapping::Off {
                None
            } else {
                self.snapshot
                    .borrow()
                    .as_ref()
                    .map(|snapshot| Mapping::new(snapshot, style))
            };
    }
    pub fn apply(&self, styles: CharacterStyles) -> CharacterStyles {
        self.mapping
            .borrow()
            .as_ref()
            .map_or(styles, |mapping| mapping.apply(styles))
    }
    pub fn theme_background(&self, styles: CharacterStyles) -> bool {
        self.mapping
            .borrow()
            .as_ref()
            .is_some_and(|mapping| mapping.theme_background(styles))
    }
}

/// Expand compact handoff associations only at replay, without repeating the snapshot table.
/// Ordinary captures and legacy transfers never enter this path.
pub fn handoff_replay(
    contents: String,
    metadata: Option<yinz_types::agent::theme::AgentThemeHandoff>,
) -> Vec<u8> {
    use base64::Engine;
    let Some(mut metadata) = metadata.filter(|m| m.valid(&contents)) else {
        return contents.into_bytes();
    };
    let associations = std::mem::take(&mut metadata.associations);
    let Ok(bytes) = serde_json::to_vec(&metadata) else {
        return contents.into_bytes();
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut output = format!("\x1b]777;yinz-agent-history;begin;{encoded}\x1b\\").into_bytes();
    let mut offset = 0;
    for association in associations {
        output.extend_from_slice(&contents.as_bytes()[offset..association.offset]);
        let palette = association
            .palette
            .map_or_else(|| "none".into(), |i| i.to_string());
        output.extend_from_slice(
            format!("\x1b]777;yinz-agent-history;select;{palette}\x1b\\").as_bytes(),
        );
        offset = association.offset;
    }
    output.extend_from_slice(&contents.as_bytes()[offset..]);
    output.extend_from_slice(b"\x1b]777;yinz-agent-history;end\x1b\\");
    output
}

/// Legacy transfers have no associations: preserve their UTF-8 suffix behavior.
pub fn fit_legacy_contents(mut contents: String, budget: usize) -> String {
    let mut cut = contents.len().saturating_sub(budget);
    while !contents.is_char_boundary(cut) {
        cut += 1;
    }
    contents.drain(..cut);
    contents
}
