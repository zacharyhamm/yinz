//! Helpers shared by the plugin's tests: a `ModeInfo` off the shipped config, and a way to read
//! a rendered line back as text.

use zellij_tile::prelude::*;
use zellij_utils::input::config::Config;

use crate::LinePart;

/// A `ModeInfo` in `mode`, carrying the keybinds the shipped config actually gives it.
pub fn with_shipped_keybinds(mode: InputMode) -> ModeInfo {
    let config = Config::from_default_assets().expect("the shipped config parses");
    let mut mode_info = ModeInfo {
        mode,
        ..ModeInfo::default()
    };
    mode_info.update_keybinds(config.keybinds);
    mode_info
}

/// Strip SGR styling, and turn the one-line layout's serialized UI components back into
/// text - a ribbon as `[text]`.
pub fn plain_text(line_part: LinePart) -> String {
    let rendered = line_part.to_string();
    let component = regex::Regex::new(r"\x1bPz(text|ribbon);([^\x1b]*)\x1b\\").unwrap();
    let decoded = component.replace_all(&rendered, |caps: &regex::Captures| {
        // The payload is `[xzd]*` style flags, `i,j$` index groups, then the text as
        // decimal bytes.
        let payload = caps[2].trim_start_matches(['x', 'z', 'd']);
        let bytes = payload.rsplit('$').next().unwrap_or_default();
        let text = bytes
            .split(',')
            .filter_map(|byte| byte.parse::<u8>().ok())
            .collect::<Vec<_>>();
        let text = String::from_utf8_lossy(&text);
        if &caps[1] == "ribbon" {
            format!("[{}]", text)
        } else {
            text.to_string()
        }
    });
    let sgr = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    sgr.replace_all(&decoded, "").to_string()
}
