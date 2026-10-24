//! Key parsing and CLI diagnostics.
use std::str::FromStr;
use yinz_types::data::PaneId;
use yinz_types::input::actions::Action;
use yinz_types::keys::KeyWithModifier;

pub(super) fn send_keys(keys: Vec<String>, pane_id: Option<String>) -> Result<Vec<Action>, String> {
    let mut actions = Vec::new();

    for (index, key_str) in keys.iter().enumerate() {
        let key = KeyWithModifier::from_str(key_str).map_err(|e| {
            let suggestion = suggest_key_fix(key_str);
            format!(
                "Invalid key at position {}: \"{}\"\n  Error: {}\n{}",
                index + 1,
                key_str,
                e,
                suggestion
            )
        })?;

        let bytes = yinz_key_codec::keys::serialize_kitty(&key)
            .map(|s| s.into_bytes())
            .unwrap_or_default();

        match &pane_id {
            Some(pane_id_str) => {
                let parsed_pane_id = PaneId::from_str(pane_id_str)
                        .map_err(|_| format!(
                            "Malformed pane id: {}, expecting either a bare integer (eg. 1), a terminal pane id (eg. terminal_1) or a plugin pane id (eg. plugin_1)",
                            pane_id_str
                        ))?;
                actions.push(Action::WriteToPaneId {
                    bytes,
                    pane_id: parsed_pane_id,
                });
            },
            None => {
                actions.push(Action::Write {
                    key_with_modifier: Some(key),
                    bytes,
                    is_kitty_keyboard_protocol: true,
                });
            },
        }
    }

    Ok(actions)
}

fn suggest_key_fix(key_str: &str) -> String {
    if key_str.contains('-') {
        return "  Hint: Use spaces instead of hyphens (e.g., \"Ctrl a\" not \"Ctrl-a\")"
            .to_string();
    }

    if key_str.trim().is_empty() {
        return "  Hint: Key string cannot be empty".to_string();
    }

    let parts: Vec<&str> = key_str.split_whitespace().collect();
    if parts.len() > 1 {
        for part in &parts[..parts.len() - 1] {
            let lower = part.to_ascii_lowercase();
            if lower.starts_with("ctr") && lower != "ctrl" {
                return format!("  Hint: Did you mean \"Ctrl\" instead of \"{}\"?", part);
            }
            if !matches!(lower.as_str(), "ctrl" | "alt" | "shift" | "super") {
                return "  Hint: Valid modifiers are: Ctrl, Alt, Shift, Super".to_string();
            }
        }
    }

    "  Hint: Use format like \"Ctrl a\", \"Alt Shift F1\", or \"Enter\"".to_string()
}
